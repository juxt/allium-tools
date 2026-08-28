//! Linear rational arithmetic (spike, SD-2). A dependency-free decision procedure
//! for the satisfiability of a conjunction of linear (in)equalities over rational
//! unknowns: Gaussian elimination of the equalities, then Fourier–Motzkin on the
//! inequalities, with a rational witness on SAT. No Z3, so the single-static-binary
//! constraint holds. Rationals (i128) mean UNSAT is exact; a SAT witness may be
//! fractional (a sound over-approximation of the integer/minor-unit reality, noted
//! where it matters).

use std::collections::BTreeMap;

/// A rational number, kept reduced with a positive denominator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rat {
    n: i128,
    d: i128,
}

impl Default for Rat {
    fn default() -> Rat {
        Rat { n: 0, d: 1 }
    }
}

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a.max(1)
}

impl Rat {
    pub fn new(n: i128, d: i128) -> Rat {
        assert!(d != 0, "zero denominator");
        let s = if d < 0 { -1 } else { 1 };
        let g = gcd(n, d);
        Rat { n: s * n / g, d: s * d / g }
    }
    pub fn int(n: i64) -> Rat {
        Rat { n: n as i128, d: 1 }
    }
    pub fn zero() -> Rat {
        Rat { n: 0, d: 1 }
    }
    pub fn is_zero(&self) -> bool {
        self.n == 0
    }
    pub fn add(self, o: Rat) -> Rat {
        Rat::new(self.n * o.d + o.n * self.d, self.d * o.d)
    }
    pub fn sub(self, o: Rat) -> Rat {
        Rat::new(self.n * o.d - o.n * self.d, self.d * o.d)
    }
    pub fn mul(self, o: Rat) -> Rat {
        Rat::new(self.n * o.n, self.d * o.d)
    }
    pub fn div(self, o: Rat) -> Rat {
        Rat::new(self.n * o.d, self.d * o.n)
    }
    pub fn neg(self) -> Rat {
        Rat { n: -self.n, d: self.d }
    }
    /// Sign: <0, =0, >0 as -1/0/1.
    fn sign(&self) -> i32 {
        self.n.signum() as i32
    }
    pub fn to_f64(self) -> f64 {
        self.n as f64 / self.d as f64
    }
    pub fn show(&self) -> String {
        if self.d == 1 {
            self.n.to_string()
        } else {
            format!("{}/{}", self.n, self.d)
        }
    }
}

/// A linear form `sum(coeff * var) + c`.
#[derive(Clone, Debug, Default)]
pub struct Lin {
    pub terms: BTreeMap<String, Rat>,
    pub c: Rat,
}

impl Lin {
    pub fn konst(c: Rat) -> Lin {
        Lin { terms: BTreeMap::new(), c }
    }
    pub fn var(name: &str) -> Lin {
        let mut t = BTreeMap::new();
        t.insert(name.to_string(), Rat::int(1));
        Lin { terms: t, c: Rat::zero() }
    }
    fn insert(&mut self, v: &str, k: Rat) {
        let e = self.terms.entry(v.to_string()).or_insert_with(Rat::zero);
        *e = e.add(k);
        if e.is_zero() {
            self.terms.remove(v);
        }
    }
    pub fn add(&self, o: &Lin) -> Lin {
        let mut r = self.clone();
        for (v, k) in &o.terms {
            r.insert(v, *k);
        }
        r.c = r.c.add(o.c);
        r
    }
    pub fn sub(&self, o: &Lin) -> Lin {
        self.add(&o.scale(Rat::int(-1)))
    }
    pub fn scale(&self, k: Rat) -> Lin {
        let mut r = Lin { terms: BTreeMap::new(), c: self.c.mul(k) };
        if !k.is_zero() {
            for (v, c) in &self.terms {
                r.terms.insert(v.clone(), c.mul(k));
            }
        }
        r
    }
    /// Replace `v` by `repl` throughout.
    fn subst(&self, v: &str, repl: &Lin) -> Lin {
        match self.terms.get(v) {
            None => self.clone(),
            Some(&k) => {
                let mut base = self.clone();
                base.terms.remove(v);
                base.add(&repl.scale(k))
            }
        }
    }
    fn eval(&self, assign: &BTreeMap<String, Rat>) -> Rat {
        let mut acc = self.c;
        for (v, k) in &self.terms {
            acc = acc.add(k.mul(*assign.get(v).copied().get_or_insert(Rat::zero())));
        }
        acc
    }
    fn is_const(&self) -> bool {
        self.terms.is_empty()
    }
}

/// Relation of a linear form to zero.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rel {
    Eq,
    Le,
    Lt,
}

/// A labelled constraint `lin  <rel>  0`.
#[derive(Clone, Debug)]
pub struct Con {
    pub lin: Lin,
    pub rel: Rel,
    pub label: String,
}

impl Con {
    pub fn new(lin: Lin, rel: Rel, label: impl Into<String>) -> Con {
        Con { lin, rel, label: label.into() }
    }
}

pub enum Outcome {
    Sat(BTreeMap<String, Rat>),
    Unsat,
}

/// Decide the satisfiability of a conjunction of constraints, with a witness on SAT.
pub fn solve(cons: &[Con]) -> Outcome {
    // 1. Gaussian elimination of equalities. `subs[v] = Lin` keeps each eliminated
    //    variable expressed in terms of the not-yet-eliminated ones.
    let mut subs: Vec<(String, Lin)> = Vec::new();
    let mut ineqs: Vec<(Lin, Rel)> =
        cons.iter().filter(|c| c.rel != Rel::Eq).map(|c| (c.lin.clone(), c.rel)).collect();
    let mut eqs: Vec<Lin> = cons.iter().filter(|c| c.rel == Rel::Eq).map(|c| c.lin.clone()).collect();

    while let Some(mut eq) = eqs.pop() {
        // Reduce by known substitutions.
        for (v, repl) in &subs {
            eq = eq.subst(v, repl);
        }
        if eq.is_const() {
            if !eq.c.is_zero() {
                return Outcome::Unsat;
            }
            continue;
        }
        let (pv, pk) = eq.terms.iter().next().map(|(v, k)| (v.clone(), *k)).unwrap();
        // v = -(eq - pk*v)/pk
        let mut rest = eq.clone();
        rest.terms.remove(&pv);
        let repl = rest.scale(Rat::int(-1).div(pk));
        // Forward-substitute into remaining eqs, ineqs, and prior subs.
        for e in eqs.iter_mut() {
            *e = e.subst(&pv, &repl);
        }
        for (l, _) in ineqs.iter_mut() {
            *l = l.subst(&pv, &repl);
        }
        for (_, r) in subs.iter_mut() {
            *r = r.subst(&pv, &repl);
        }
        subs.push((pv, repl));
    }

    // 2. Fourier–Motzkin over the free variables of the inequalities.
    let mut vars: Vec<String> = {
        let mut s: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (l, _) in &ineqs {
            s.extend(l.terms.keys().cloned());
        }
        s.into_iter().collect()
    };
    let free = match fm(&ineqs, &mut vars) {
        Some(a) => a,
        None => return Outcome::Unsat,
    };

    // 3. Rebuild the eliminated variables from the free assignment.
    let mut assign = free;
    for (v, repl) in subs.iter().rev() {
        let val = repl.eval(&assign);
        assign.insert(v.clone(), val);
    }
    Outcome::Sat(assign)
}

/// Fourier–Motzkin with witness. Returns an assignment to `vars` satisfying every
/// inequality, or `None` if the projection is empty.
fn fm(ineqs: &[(Lin, Rel)], vars: &mut Vec<String>) -> Option<BTreeMap<String, Rat>> {
    let x = match vars.first().cloned() {
        None => {
            // Base: every constraint is now constant; check it holds.
            for (l, rel) in ineqs {
                let s = l.c.sign();
                let ok = match rel {
                    Rel::Le => s <= 0,
                    Rel::Lt => s < 0,
                    Rel::Eq => s == 0,
                };
                if !ok {
                    return None;
                }
            }
            return Some(BTreeMap::new());
        }
        Some(v) => v,
    };
    let rest_vars: Vec<String> = vars[1..].to_vec();

    // Partition by the coefficient of x. lower: x >= bound; upper: x <= bound.
    let mut lowers: Vec<(Lin, Rel)> = Vec::new(); // bound expr, strictness
    let mut uppers: Vec<(Lin, Rel)> = Vec::new();
    let mut zero: Vec<(Lin, Rel)> = Vec::new();
    for (l, rel) in ineqs {
        match l.terms.get(&x).copied() {
            None => zero.push((l.clone(), *rel)),
            Some(a) => {
                // a*x + rest <rel> 0  =>  x <rel'> -rest/a
                let mut rest = l.clone();
                rest.terms.remove(&x);
                let bound = rest.scale(Rat::int(-1).div(a)); // x <rel'> bound
                if a.sign() > 0 {
                    uppers.push((bound, *rel)); // x <= / < bound
                } else {
                    lowers.push((bound, *rel)); // x >= / > bound
                }
            }
        }
    }

    // Project: for every (lower L, upper U) require L <= U (strict if either strict).
    let mut projected = zero;
    for (lo, lrel) in &lowers {
        for (up, urel) in &uppers {
            let lin = lo.sub(up); // L - U  <=/< 0
            let rel = if *lrel == Rel::Lt || *urel == Rel::Lt { Rel::Lt } else { Rel::Le };
            projected.push((lin, rel));
        }
    }

    let mut inner_vars = rest_vars;
    let assign = fm(&projected, &mut inner_vars)?;

    // Pick a value for x within [max lower, min upper].
    let mut lo: Option<(Rat, bool)> = None; // (value, strict)
    for (bound, rel) in &lowers {
        let v = bound.eval(&assign);
        let strict = *rel == Rel::Lt;
        lo = Some(match lo {
            None => (v, strict),
            Some((cur, cs)) => {
                if v.sub(cur).sign() > 0 {
                    (v, strict)
                } else if v == cur {
                    (cur, cs || strict)
                } else {
                    (cur, cs)
                }
            }
        });
    }
    let mut hi: Option<(Rat, bool)> = None;
    for (bound, rel) in &uppers {
        let v = bound.eval(&assign);
        let strict = *rel == Rel::Lt;
        hi = Some(match hi {
            None => (v, strict),
            Some((cur, cs)) => {
                if v.sub(cur).sign() < 0 {
                    (v, strict)
                } else if v == cur {
                    (cur, cs || strict)
                } else {
                    (cur, cs)
                }
            }
        });
    }
    let xv = match (lo, hi) {
        (None, None) => Rat::zero(),
        (Some((l, _)), None) => l.add(Rat::int(1)),
        (None, Some((u, _))) => u.sub(Rat::int(1)),
        (Some((l, _)), Some((u, _))) => {
            // A midpoint lies strictly between when l < u; equal non-strict bounds pin it.
            if l == u {
                l
            } else {
                l.add(u).div(Rat::int(2))
            }
        }
    };
    let mut out = assign;
    out.insert(x, xv);
    Some(out)
}

/// A minimal UNSAT core by greedy removal: drop each constraint; keep it only if its
/// removal restores satisfiability. Mirrors the SAT-layer core in `analyse`.
pub fn unsat_core(cons: &[Con]) -> Vec<String> {
    let mut active = vec![true; cons.len()];
    let subset = |active: &[bool]| -> Vec<Con> {
        cons.iter().enumerate().filter(|(i, _)| active[*i]).map(|(_, c)| c.clone()).collect()
    };
    for k in 0..cons.len() {
        active[k] = false;
        if let Outcome::Sat(_) = solve(&subset(&active)) {
            active[k] = true;
        }
    }
    cons.iter().enumerate().filter(|(i, _)| active[*i]).map(|(_, c)| c.label.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(name: &str) -> Lin {
        Lin::var(name)
    }
    fn eq(a: Lin, b: Lin, l: &str) -> Con {
        Con::new(a.sub(&b), Rel::Eq, l)
    }
    fn le(a: Lin, b: Lin, l: &str) -> Con {
        Con::new(a.sub(&b), Rel::Le, l)
    }
    fn lt(a: Lin, b: Lin, l: &str) -> Con {
        Con::new(a.sub(&b), Rel::Lt, l)
    }
    fn val(o: &Outcome, name: &str) -> Rat {
        match o {
            Outcome::Sat(m) => *m.get(name).unwrap(),
            Outcome::Unsat => panic!("unsat"),
        }
    }

    #[test]
    fn equality_chain_solves() {
        // x = y + 1, y = 2  => x = 3
        let cons = vec![
            eq(v("x"), v("y").add(&Lin::konst(Rat::int(1))), "e1"),
            eq(v("y"), Lin::konst(Rat::int(2)), "e2"),
        ];
        let o = solve(&cons);
        assert_eq!(val(&o, "x"), Rat::int(3));
        assert_eq!(val(&o, "y"), Rat::int(2));
    }

    #[test]
    fn contradictory_bounds_are_unsat() {
        // x >= 5 and x <= 3
        let cons = vec![le(Lin::konst(Rat::int(5)), v("x"), "lo"), le(v("x"), Lin::konst(Rat::int(3)), "hi")];
        assert!(matches!(solve(&cons), Outcome::Unsat));
        let core = unsat_core(&cons);
        assert!(core.contains(&"lo".to_string()) && core.contains(&"hi".to_string()));
    }

    #[test]
    fn feasible_interval_gives_witness() {
        let cons = vec![le(Lin::konst(Rat::zero()), v("x"), "lo"), le(v("x"), Lin::konst(Rat::int(10)), "hi")];
        let o = solve(&cons);
        let x = val(&o, "x");
        assert!(x.to_f64() >= 0.0 && x.to_f64() <= 10.0);
    }

    #[test]
    fn balance_roll_forces_negative_principal_when_balance_grows() {
        // b1 = b0 - p0 ; and b1 > b0 (negated monotonicity)  =>  SAT with p0 < 0.
        let cons = vec![
            eq(v("b1"), v("b0").sub(&v("p0")), "roll"),
            lt(v("b0"), v("b1"), "not_monotone"),
        ];
        let o = solve(&cons);
        assert!(val(&o, "p0").to_f64() < 0.0, "p0 must be negative");
    }
}
