//! A small, dependency-free SAT engine: Tseitin CNF encoding + DPLL. It lets the
//! consistency and feasibility checks scale past brute-force enumeration (which is
//! 2^atoms and capped at 16) to hundreds of boolean atoms, without linking Z3 — so the
//! single-static-binary constraint is preserved.
//!
//! Atoms are the leaf boolean terms of a predicate (an application like `cleared(r)`, a
//! field, or a comparison), identified by their canonical string. Quantifier-free boolean
//! structure (`and`/`or`/`not`/`implies`) is encoded exactly; a quantified subterm is
//! treated as an opaque atom (sound: it never manufactures a false UNSAT for our use,
//! where UNSAT is the alarm — an opaque atom only adds freedom).

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

use crate::analyse::canon;
use crate::expr::{BinOp, Expr, UnOp};

/// Cap on total variables (atoms + Tseitin aux) to bound pathological inputs.
const MAX_VARS: usize = 2000;

/// Is `e` a boolean-valued expression, given the set of boolean state/relation names? Used to decide
/// whether `A = B` is a biconditional between two booleans (which must be encoded) or an arithmetic
/// equality (which stays an opaque atom, handled by the LRA path). Getting this wrong in the unsafe
/// direction only ever adds freedom, so the default (no names) keeps every `=` opaque as before.
pub(crate) fn is_bool_valued(e: &Expr, bool_names: &HashSet<String>) -> bool {
    match e {
        Expr::Name(s) => s == "true" || s == "false" || bool_names.contains(s),
        Expr::App { head, .. } => matches!(&**head, Expr::Name(h) if bool_names.contains(h)),
        Expr::Field { name, .. } => bool_names.contains(name),
        Expr::Unary { op: UnOp::Not, .. } => true,
        Expr::Binary { op, .. } => matches!(
            op,
            BinOp::And | BinOp::Or | BinOp::Implies | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge
                | BinOp::Eq | BinOp::Ne | BinOp::In
        ),
        _ => false,
    }
}

struct CnfBuilder<'a> {
    clauses: Vec<Vec<i32>>,
    atom_index: HashMap<String, i32>, // atom canon -> var id (1-based, positive)
    nvars: i32,
    overflow: bool,
    bool_names: &'a HashSet<String>,
    /// Enum-typed observable name -> its declared value set. An equality `status(e) = paid` over one of
    /// these is a proposition, and `status` takes EXACTLY ONE value, so `status=created` and `status=paid`
    /// are mutually exclusive and one of them holds. Without this the encoder treats each `=` as an
    /// independent opaque atom and misses the state-machine structure.
    enum_vals: &'a HashMap<String, Vec<String>>,
    /// Enum-equality groups that appeared (canon(lhs) -> (lhs expr, value set)), for the exactly-one axiom.
    enum_groups: HashMap<String, (Expr, Vec<String>)>,
    /// A variable pinned true by a unit clause, so the boolean literals `true`/`false` encode as the
    /// constants ⊤/⊥ rather than fresh free atoms (else `logged = true` would let the solver pick
    /// `true = false` and manufacture a spurious model).
    true_var: Option<i32>,
}

impl<'a> CnfBuilder<'a> {
    fn new(bool_names: &'a HashSet<String>, enum_vals: &'a HashMap<String, Vec<String>>) -> Self {
        CnfBuilder { clauses: Vec::new(), atom_index: HashMap::new(), nvars: 0, overflow: false, bool_names, enum_vals, enum_groups: HashMap::new(), true_var: None }
    }

    /// A literal that is always true (pinned by a unit clause, created once). `false` is its negation.
    fn true_literal(&mut self) -> i32 {
        if let Some(v) = self.true_var {
            return v;
        }
        let v = self.fresh();
        self.clauses.push(vec![v]);
        self.true_var = Some(v);
        v
    }

    /// If `e` is an application (or bare name) whose head is a declared enum observable, its name.
    fn enum_head(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::App { head, .. } => match &**head {
                Expr::Name(h) if self.enum_vals.contains_key(h) => Some(h.clone()),
                _ => None,
            },
            Expr::Name(n) if self.enum_vals.contains_key(n) => Some(n.clone()),
            _ => None,
        }
    }

    /// Emit the exactly-one-value axiom for every enum-equality group that appeared: at least one value
    /// holds, and no two hold at once. Makes an enum observable a genuine one-of-N state.
    fn finalize_enums(&mut self) {
        let groups: Vec<(Expr, Vec<String>)> = self.enum_groups.values().cloned().collect();
        for (lhs, vals) in groups {
            let lits: Vec<i32> = vals
                .iter()
                .map(|v| {
                    let atom = format!("{} = {}", canon(&lhs), v);
                    self.atom(atom)
                })
                .collect();
            if lits.len() < 2 {
                continue;
            }
            self.clauses.push(lits.clone()); // at least one
            for i in 0..lits.len() {
                for j in (i + 1)..lits.len() {
                    self.clauses.push(vec![-lits[i], -lits[j]]); // at most one
                }
            }
        }
    }

    fn fresh(&mut self) -> i32 {
        self.nvars += 1;
        if self.nvars as usize > MAX_VARS {
            self.overflow = true;
        }
        self.nvars
    }

    fn atom(&mut self, name: String) -> i32 {
        if let Some(&v) = self.atom_index.get(&name) {
            return v;
        }
        let v = self.fresh();
        self.atom_index.insert(name, v);
        v
    }

    /// Tseitin: return a literal equivalent to `e`, adding its defining clauses.
    fn encode(&mut self, e: &Expr) -> i32 {
        match e {
            // The boolean literals encode as the constants ⊤/⊥, not fresh atoms.
            Expr::Name(s) if s == "true" => self.true_literal(),
            Expr::Name(s) if s == "false" => -self.true_literal(),
            Expr::Unary { op: UnOp::Not, e } => -self.encode(e),
            Expr::Binary { op: BinOp::And, lhs, rhs } => {
                let a = self.encode(lhs);
                let b = self.encode(rhs);
                let x = self.fresh();
                // x <-> a & b
                self.clauses.push(vec![-x, a]);
                self.clauses.push(vec![-x, b]);
                self.clauses.push(vec![-a, -b, x]);
                x
            }
            Expr::Binary { op: BinOp::Or, lhs, rhs } => {
                let a = self.encode(lhs);
                let b = self.encode(rhs);
                let x = self.fresh();
                // x <-> a | b
                self.clauses.push(vec![-x, a, b]);
                self.clauses.push(vec![-a, x]);
                self.clauses.push(vec![-b, x]);
                x
            }
            Expr::Binary { op: BinOp::Implies, lhs, rhs } => {
                let a = self.encode(lhs);
                let b = self.encode(rhs);
                let x = self.fresh();
                // x <-> (!a | b)
                self.clauses.push(vec![-x, -a, b]);
                self.clauses.push(vec![a, x]);
                self.clauses.push(vec![-b, x]);
                x
            }
            // Boolean `=`/`<>` is a biconditional/xor between two booleans and must be encoded, not
            // treated as one opaque atom (the latter silently drops the constraint `a = b`). Arithmetic
            // equality (either side non-boolean) stays opaque here and is handled by the LRA path.
            Expr::Binary { op: op @ (BinOp::Eq | BinOp::Ne), lhs, rhs }
                if is_bool_valued(lhs, self.bool_names) && is_bool_valued(rhs, self.bool_names) =>
            {
                let a = self.encode(lhs);
                let b = self.encode(rhs);
                let x = self.fresh();
                if matches!(op, BinOp::Eq) {
                    // x <-> (a <-> b)
                    self.clauses.push(vec![-x, -a, b]);
                    self.clauses.push(vec![-x, a, -b]);
                    self.clauses.push(vec![x, -a, -b]);
                    self.clauses.push(vec![x, a, b]);
                } else {
                    // x <-> (a xor b)
                    self.clauses.push(vec![-x, a, b]);
                    self.clauses.push(vec![-x, -a, -b]);
                    self.clauses.push(vec![x, -a, b]);
                    self.clauses.push(vec![x, a, -b]);
                }
                x
            }
            // An equality `enum_obs(args) = value` is a proposition; register its group so the exactly-one
            // axiom is added at finalize, then encode the whole equality as one atom.
            Expr::Binary { op: BinOp::Eq, lhs, rhs }
                if self.enum_head(lhs).is_some() && matches!(&**rhs, Expr::Name(_)) =>
            {
                let obs = self.enum_head(lhs).unwrap();
                let vals = self.enum_vals[&obs].clone();
                self.enum_groups.entry(canon(lhs)).or_insert(((**lhs).clone(), vals));
                self.atom(canon(e))
            }
            // `x in { a, b, c }` — membership in a finite enum set. Encode as `x = a or x = b or x = c`;
            // each equality flows through the arm above, registering the exactly-one axiom for `x`.
            Expr::Binary { op: BinOp::In, lhs, rhs } if matches!(&**rhs, Expr::SetLit(_)) => {
                let s = match &**rhs {
                    Expr::SetLit(s) => s.clone(),
                    _ => unreachable!(),
                };
                // The set literal text may retain its enclosing braces; strip them and any per-element
                // whitespace/braces so an element is the bare tag (`admin`, not `admin }`).
                let elems: Vec<String> = s
                    .split(|c| c == ',' || c == '|')
                    .map(|t| t.trim().trim_matches(|c| c == '{' || c == '}').trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect();
                if elems.is_empty() {
                    return -self.true_literal(); // empty set: membership is false
                }
                let eq = |el: &str| Expr::Binary {
                    op: BinOp::Eq,
                    lhs: lhs.clone(),
                    rhs: Box::new(Expr::Name(el.to_string())),
                };
                let disj = elems[1..].iter().fold(eq(&elems[0]), |acc, el| Expr::Binary {
                    op: BinOp::Or,
                    lhs: Box::new(acc),
                    rhs: Box::new(eq(el)),
                });
                self.encode(&disj)
            }
            atom => self.atom(canon(atom)),
        }
    }

    fn assert(&mut self, e: &Expr) {
        let l = self.encode(e);
        self.clauses.push(vec![l]);
    }
}

/// DPLL with unit propagation. `assign[v]`: 0 unassigned, 1 true, -1 false (1-based).
fn dpll(clauses: &[Vec<i32>], assign: &mut [i8]) -> bool {
    // Unit propagation to a fixpoint.
    loop {
        let mut changed = false;
        for c in clauses {
            let mut unassigned = 0i32;
            let mut count = 0;
            let mut satisfied = false;
            for &l in c {
                let v = l.unsigned_abs() as usize;
                let want = l > 0;
                match assign[v] {
                    0 => {
                        unassigned = l;
                        count += 1;
                    }
                    a => {
                        if (a == 1) == want {
                            satisfied = true;
                            break;
                        }
                    }
                }
            }
            if satisfied {
                continue;
            }
            if count == 0 {
                return false; // conflict
            }
            if count == 1 {
                let v = unassigned.unsigned_abs() as usize;
                assign[v] = if unassigned > 0 { 1 } else { -1 };
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // Choose an unassigned variable and branch.
    match (1..assign.len()).find(|&i| assign[i] == 0) {
        None => true,
        Some(v) => {
            for val in [1i8, -1] {
                let mut a2 = assign.to_vec();
                a2[v] = val;
                if dpll(clauses, &mut a2) {
                    assign.copy_from_slice(&a2);
                    return true;
                }
            }
            false
        }
    }
}

/// Is the conjunction of `exprs` satisfiable? Returns a witness (atom -> bool) if so, or
/// `None` if UNSAT. `None` is also returned on variable overflow (treated conservatively
/// as "cannot certify SAT"); callers should note the cap.
pub fn satisfiable(exprs: &[&Expr], bool_names: &HashSet<String>) -> Option<BTreeMap<String, bool>> {
    satisfiable_enum(exprs, bool_names, &HashMap::new())
}

/// As [`satisfiable`], plus the exactly-one-value axiom for each declared enum-typed observable
/// (`enum_vals`: observable name -> its value set, including any primed post-state versions).
pub fn satisfiable_enum(
    exprs: &[&Expr],
    bool_names: &HashSet<String>,
    enum_vals: &HashMap<String, Vec<String>>,
) -> Option<BTreeMap<String, bool>> {
    let mut b = CnfBuilder::new(bool_names, enum_vals);
    for e in exprs {
        b.assert(e);
    }
    b.finalize_enums();
    if b.overflow {
        return None;
    }
    let mut assign = vec![0i8; (b.nvars + 1) as usize];
    if dpll(&b.clauses, &mut assign) {
        let mut m = BTreeMap::new();
        for (name, &vid) in &b.atom_index {
            m.insert(name.clone(), assign[vid as usize] == 1);
        }
        Some(m)
    } else {
        None
    }
}

/// Render a witness assignment compactly, sorted by atom name.
pub fn describe(m: &BTreeMap<String, bool>) -> String {
    m.iter().map(|(k, v)| format!("{k}={}", if *v { "T" } else { "F" })).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::parse_predicate;

    fn p(s: &str) -> Expr {
        parse_predicate(s).0
    }

    #[test]
    fn detects_unsat_chain() {
        // a->b, b->!c, and a & c  =>  UNSAT
        let e1 = p("a implies b");
        let e2 = p("b implies not c");
        let e3 = p("a and c");
        assert!(satisfiable(&[&e1, &e2, &e3], &std::collections::HashSet::new()).is_none());
    }

    #[test]
    fn boolean_equality_is_a_biconditional() {
        // `a = b` between two BOOLEAN names plus `a and not b` is a contradiction. It must be UNSAT
        // when a,b are known boolean (biconditional encoded), and remain SAT when they are not (an
        // opaque atom, the arithmetic-equality case handled elsewhere).
        let eq = p("a = b");
        let contra = p("a and not b");
        let mut bools = HashSet::new();
        bools.insert("a".to_string());
        bools.insert("b".to_string());
        assert!(satisfiable(&[&eq, &contra], &bools).is_none(), "boolean a=b should be a biconditional");
        assert!(satisfiable(&[&eq, &contra], &HashSet::new()).is_some(), "non-boolean = stays opaque");
    }

    #[test]
    fn finds_sat_witness() {
        let e1 = p("a implies b");
        let e2 = p("a");
        let m = satisfiable(&[&e1, &e2], &std::collections::HashSet::new()).unwrap();
        assert_eq!(m.get("a"), Some(&true));
        assert_eq!(m.get("b"), Some(&true));
    }

    #[test]
    fn scales_past_sixteen_atoms() {
        // 40 independent implications a_i -> b_i, plus one contradiction on the last pair.
        let mut exprs = Vec::new();
        for i in 0..40 {
            exprs.push(p(&format!("a{i} implies b{i}")));
        }
        exprs.push(p("a39"));
        exprs.push(p("a39 implies b39"));
        exprs.push(p("b39 implies not c"));
        exprs.push(p("c"));
        let refs: Vec<&Expr> = exprs.iter().collect();
        assert!(satisfiable(&refs, &std::collections::HashSet::new()).is_none()); // 80+ atoms, still decided
    }
}
