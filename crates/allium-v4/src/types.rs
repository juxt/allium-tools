//! v4 dimensioned-scalar checking (spike, SD-1). A numeric type may carry a
//! *dimension* — an open nominal tag that partitions otherwise-identical numbers
//! into non-combining families: `Money(gbp)` vs `Money(usd)`, `Mass(hectogram)` vs
//! `Mass(kilogram)`. The tag is a phantom: it lives only here, at check time, and is
//! erased before analyse/sat run, so the arithmetic tier pays nothing for it.
//!
//! The rules are the additive tier plus scalar scaling (SD-1): `+`/`-`/comparison
//! require a shared dimension; `*` lets a dimensionless scalar (a `Rate`, a count, a
//! literal) scale a dimensioned quantity but forbids two dimensioned operands; and the
//! literal `0` is the dimension-polymorphic additive identity, so `balance = 0` checks.
//!
//! A cross-dimension combination is an ERROR — it is the K55 category error the
//! specimen exists to catch. Where either side's dimension is unknown (undeclared
//! types, bound variables), nothing is raised: the check never manufactures a false
//! alarm, matching the crate's "never a false UNSAT" posture.

use std::collections::HashMap;

use crate::ast::{ItemKind, Module};
use crate::diagnostic::Diagnostic;
use crate::expr::{parse_predicate, BinOp, Expr, UnOp};
use crate::span::Span;

/// A dimension: the nominal family a number belongs to. `Scalar` is dimensionless
/// (rates, counts, literals) and scales anything.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Dim {
    Scalar,
    /// Money in some currency (`""` when the type left it unspecified).
    Money(String),
    /// A named physical dimension with an optional unit, e.g. `Mass(hectogram)`.
    Named(String, String),
}

impl Dim {
    fn desc(&self) -> String {
        match self {
            Dim::Scalar => "a scalar".to_string(),
            Dim::Money(c) if c.is_empty() => "money".to_string(),
            Dim::Money(c) => format!("money({c})"),
            Dim::Named(h, u) if u.is_empty() => h.clone(),
            Dim::Named(h, u) => format!("{h}({u})"),
        }
    }
}

/// The type of an expression as far as the dimension check cares.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Ty {
    Bool,
    /// An entity/opaque sort (equality-comparable, not arithmetic).
    Ent(String),
    Num(Dim),
    /// A numeric literal. Dimension-polymorphic: it takes its dimension from the operand it
    /// combines with (`fee <= 10` reads `10` as money; `days <= 10` as days), and acts as a
    /// dimensionless scalar under multiplication (`0.02 * balance`). Generalises the old
    /// `0`-is-polymorphic rule to every literal, so a dimensioned constant needs no special
    /// syntax and works for every scalar family, not just money.
    Lit,
    /// Undeclared / unresolved: suppresses all dimension errors involving it.
    Unknown,
}

/// Parse a declared type's source text into a [`Ty`]. Head + optional `(unit)` arg.
fn parse_ty(text: &str) -> Ty {
    let t = text.trim();
    if t.is_empty() {
        return Ty::Unknown;
    }
    let (head, arg) = match t.find('(') {
        Some(i) => {
            let close = t.rfind(')').unwrap_or(t.len());
            (t[..i].trim(), t.get(i + 1..close).unwrap_or("").trim())
        }
        None => (t, ""),
    };
    let head_lc = head.to_lowercase();
    let arg = arg.to_lowercase();
    match head_lc.as_str() {
        "bool" | "boolean" => Ty::Bool,
        "money" | "amount" | "cash" => Ty::Num(Dim::Money(arg)),
        "rate" | "ratio" | "factor" | "percentage" | "percent" => Ty::Num(Dim::Scalar),
        "int" | "integer" | "nat" | "natural" | "count" | "number" | "num" | "decimal"
        | "scalar" => Ty::Num(Dim::Scalar),
        "mass" | "length" | "duration" | "weight" | "distance" | "quantity" | "volume" => {
            Ty::Num(Dim::Named(head_lc, arg))
        }
        // Anything else capitalised is an entity/opaque sort (e.g. `Period`, `Trade`).
        _ => Ty::Ent(head.to_string()),
    }
}

/// Well-typedness under the dimension rules for a whole module. Wired into `check`.
pub fn typecheck(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        // The value environment: each state/given name maps to the type of its result.
        let mut env: HashMap<String, Ty> = HashMap::new();
        for it in &d.items {
            if matches!(it.kind, ItemKind::State | ItemKind::Given) {
                if let (Some(name), Some(bsp)) = (&it.name, it.body) {
                    env.insert(name.clone(), parse_ty(bsp.slice(src)));
                }
            }
        }
        for it in &d.items {
            let spans: Vec<Span> = match it.kind {
                ItemKind::Action => it.requires.iter().chain(it.ensures.iter()).copied().collect(),
                ItemKind::Invariant
                | ItemKind::Guarantee
                | ItemKind::Fault
                | ItemKind::Requirement
                | ItemKind::Axiom
                | ItemKind::Rely
                | ItemKind::Establish
                | ItemKind::Init => it.body.iter().copied().collect(),
                _ => Vec::new(),
            };
            for span in spans {
                let (e, _) = parse_predicate(span.slice(src));
                infer(&e, &env, it.span, &mut out);
            }
        }
    }
    out
}

/// Infer an expression's [`Ty`], pushing a dimension error for every ill-formed
/// combination. Errors attach to `at` (predicate sub-expressions carry no span).
fn infer(e: &Expr, env: &HashMap<String, Ty>, at: Span, out: &mut Vec<Diagnostic>) -> Ty {
    match e {
        Expr::Int(_) | Expr::Dec(_, _) => Ty::Lit,
        Expr::Name(s) => match s.as_str() {
            "true" | "false" => Ty::Bool,
            _ => env.get(s).cloned().unwrap_or(Ty::Unknown),
        },
        Expr::App { head, args } => {
            let arg_tys: Vec<Ty> = args.iter().map(|a| infer(a, env, at, out)).collect();
            match head.as_ref() {
                // min/max return the (shared) dimension of their arguments: min(money,money)=money.
                Expr::Name(h) if (h == "min" || h == "max") && arg_tys.len() == 2 => {
                    match (&arg_tys[0], &arg_tys[1]) {
                        (Ty::Num(d), _) | (_, Ty::Num(d)) => Ty::Num(d.clone()),
                        _ => Ty::Lit,
                    }
                }
                Expr::Name(h) => env.get(h).cloned().unwrap_or(Ty::Unknown),
                _ => Ty::Unknown,
            }
        }
        Expr::Cond { cond, then_, els } => { infer(cond, env, at, out); let t = infer(then_, env, at, out); infer(els, env, at, out); t }
        Expr::Field { .. } | Expr::SetLit(_) | Expr::Error => Ty::Unknown,
        Expr::Unary { op: UnOp::Not, e } => {
            infer(e, env, at, out);
            Ty::Bool
        }
        Expr::Unary { op: UnOp::Old, e } => infer(e, env, at, out),
        Expr::Quant { body, .. } => {
            infer(body, env, at, out);
            Ty::Bool
        }
        // A sum takes the dimension of the summed term: sum of money is money.
        Expr::Sum { body, .. } => infer(body, env, at, out),
        Expr::Binary { op, lhs, rhs } => {
            let l = infer(lhs, env, at, out);
            let r = infer(rhs, env, at, out);
            match op {
                BinOp::And | BinOp::Or | BinOp::Implies | BinOp::In => Ty::Bool,
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                    if let (Some(a), Some(b)) = (dim(&l), dim(&r)) {
                        if a != b {
                            out.push(Diagnostic::error(
                                at,
                                format!(
                                    "cannot compare {} with {}: different dimensions (an explicit conversion is required)",
                                    a.desc(),
                                    b.desc()
                                ),
                            ));
                        }
                    }
                    Ty::Bool
                }
                BinOp::Add | BinOp::Sub => additive(&l, &r, op, at, out),
                BinOp::Mul => multiplicative(&l, &r, at, out),
                BinOp::Div => divisive(&l, &r, at, out),
            }
        }
    }
}

/// The dimension of a type, if it is a definite numeric family (`Lit`/`Unknown` → None:
/// they are compatible with anything and never trigger an error on their own).
fn dim(t: &Ty) -> Option<&Dim> {
    match t {
        Ty::Num(d) => Some(d),
        _ => None,
    }
}

fn additive(l: &Ty, r: &Ty, op: &BinOp, at: Span, out: &mut Vec<Diagnostic>) -> Ty {
    match (l, r) {
        (Ty::Lit, Ty::Lit) => Ty::Lit,
        (Ty::Num(d), Ty::Lit) | (Ty::Lit, Ty::Num(d)) => Ty::Num(d.clone()),
        (Ty::Num(a), Ty::Num(b)) => {
            if a != b {
                let verb = if matches!(op, BinOp::Add) { "add" } else { "subtract" };
                out.push(Diagnostic::error(
                    at,
                    format!(
                        "cannot {verb} {} and {}: different dimensions (an explicit conversion is required)",
                        a.desc(),
                        b.desc()
                    ),
                ));
                Ty::Unknown
            } else {
                Ty::Num(a.clone())
            }
        }
        _ => Ty::Unknown,
    }
}

fn divisive(l: &Ty, r: &Ty, at: Span, out: &mut Vec<Diagnostic>) -> Ty {
    match (l, r) {
        // X / scalar keeps X's dimension; money/1200 stays money.
        (Ty::Num(d), Ty::Num(Dim::Scalar)) | (Ty::Num(d), Ty::Lit) => Ty::Num(d.clone()),
        (Ty::Lit, Ty::Num(Dim::Scalar)) | (Ty::Lit, Ty::Lit) => Ty::Lit,
        // same dimension divided out -> dimensionless ratio (money/money = scalar).
        (Ty::Num(a), Ty::Num(b)) if a == b => Ty::Num(Dim::Scalar),
        (Ty::Num(a), Ty::Num(b)) => {
            out.push(Diagnostic::error(
                at,
                format!("cannot divide {} by {}: incompatible dimensions", a.desc(), b.desc()),
            ));
            Ty::Unknown
        }
        _ => Ty::Unknown,
    }
}

fn multiplicative(l: &Ty, r: &Ty, at: Span, out: &mut Vec<Diagnostic>) -> Ty {
    match (l, r) {
        (Ty::Num(Dim::Scalar), Ty::Num(d)) | (Ty::Num(d), Ty::Num(Dim::Scalar)) => Ty::Num(d.clone()),
        // A literal acts as a dimensionless scalar multiplier: `0.02 * balance` keeps money.
        (Ty::Lit, Ty::Num(d)) | (Ty::Num(d), Ty::Lit) => Ty::Num(d.clone()),
        (Ty::Lit, Ty::Lit) => Ty::Lit,
        (Ty::Num(a), Ty::Num(b)) => {
            // Two genuinely dimensioned operands: the multiplicative dimension algebra
            // is deferred (SD-1), so this is ill-formed rather than silently opaque.
            out.push(Diagnostic::error(
                at,
                format!(
                    "cannot multiply two dimensioned quantities ({} by {}): one side must be a dimensionless scalar (the multiplicative dimension algebra is not yet a construct)",
                    a.desc(),
                    b.desc()
                ),
            ));
            Ty::Unknown
        }
        _ => Ty::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use crate::check::check;

    fn errors(src: &str) -> Vec<String> {
        check(src)
            .diagnostics
            .into_iter()
            .filter(|d| d.is_error())
            .map(|d| d.message)
            .collect()
    }

    const HDR: &str = "-- allium: 4\ncomponent Book\n  entity Acct\n";

    #[test]
    fn same_currency_add_is_ok() {
        let src = format!(
            "{HDR}  observable state bal(Acct) : Money(gbp)\n  observable state fee(Acct) : Money(gbp)\n  invariant ok means every a :: bal(a) = bal(a) + fee(a)\nend\n"
        );
        assert!(errors(&src).iter().all(|m| !m.contains("dimensions")), "{:?}", errors(&src));
    }

    #[test]
    fn cross_currency_add_is_an_error() {
        let src = format!(
            "{HDR}  observable state gbp(Acct) : Money(gbp)\n  observable state usd(Acct) : Money(usd)\n  invariant bad means every a :: gbp(a) = gbp(a) + usd(a)\nend\n"
        );
        assert!(errors(&src).iter().any(|m| m.contains("cannot add money(gbp) and money(usd)")), "{:?}", errors(&src));
    }

    #[test]
    fn money_compared_to_zero_is_ok() {
        let src = format!(
            "{HDR}  observable state bal(Acct) : Money(gbp)\n  invariant z means every a :: bal(a) = 0\nend\n"
        );
        assert!(errors(&src).iter().all(|m| !m.contains("dimensions")), "{:?}", errors(&src));
    }

    #[test]
    fn rate_times_money_is_money_but_money_times_money_is_not() {
        let ok = format!(
            "{HDR}  observable state bal(Acct) : Money(gbp)\n  observable state r(Acct) : Rate\n  observable state intr(Acct) : Money(gbp)\n  invariant i means every a :: intr(a) = r(a) * bal(a)\nend\n"
        );
        assert!(errors(&ok).iter().all(|m| !m.contains("multiply")), "{:?}", errors(&ok));
        let bad = format!(
            "{HDR}  observable state bal(Acct) : Money(gbp)\n  invariant i means every a :: bal(a) = bal(a) * bal(a)\nend\n"
        );
        assert!(errors(&bad).iter().any(|m| m.contains("cannot multiply two dimensioned")), "{:?}", errors(&bad));
    }

    #[test]
    fn dimensioned_quantity_compares_to_any_literal() {
        // The general fix: a literal takes the operand's dimension, so a money cap and a mass
        // threshold are both well-formed — not just the special `0` case, and not money-specific.
        let money = format!(
            "{HDR}  observable state fee(Acct) : Money(gbp)\n  invariant cap means every a :: fee(a) <= 10\nend\n"
        );
        assert!(errors(&money).iter().all(|m| !m.contains("dimensions")), "{:?}", errors(&money));
        let mass = format!(
            "{HDR}  observable state w(Acct) : Mass(hectogram)\n  invariant lim means every a :: w(a) <= 5\nend\n"
        );
        assert!(errors(&mass).iter().all(|m| !m.contains("dimensions")), "{:?}", errors(&mass));
        // but a literal scaling still keeps dimension, and cross-dimension still errors
        let bad = format!(
            "{HDR}  observable state gbp(Acct) : Money(gbp)\n  observable state usd(Acct) : Money(usd)\n  invariant x means every a :: gbp(a) <= usd(a)\nend\n"
        );
        assert!(errors(&bad).iter().any(|m| m.contains("different dimensions")), "{:?}", errors(&bad));
    }

    #[test]
    fn literal_does_not_launder_incompatible_dimensions() {
        // Polymorphic literals must adopt a *neighbouring* dimension without becoming a bridge that
        // launders two genuinely incompatible ones. `fee + 0` is money; `fee + w` (money + mass) is
        // still a category error even with a literal-0 elsewhere in the expression.
        let ok = format!(
            "{HDR}  observable state fee(Acct) : Money(gbp)\n  observable state cap(Acct) : Money(gbp)\n  invariant c means every a :: fee(a) <= cap(a) + 0\nend\n"
        );
        assert!(errors(&ok).iter().all(|m| !m.contains("dimensions")), "{:?}", errors(&ok));
        let launder = format!(
            "{HDR}  observable state fee(Acct) : Money(gbp)\n  observable state w(Acct) : Mass(kg)\n  observable state t(Acct) : Money(gbp)\n  invariant l means every a :: t(a) <= fee(a) + w(a)\nend\n"
        );
        assert!(errors(&launder).iter().any(|m| m.contains("cannot add money(gbp) and mass(kg)")), "{:?}", errors(&launder));
    }

    #[test]
    fn compare_money_with_rate_is_an_error() {
        let src = format!(
            "{HDR}  observable state bal(Acct) : Money(gbp)\n  observable state r(Acct) : Rate\n  invariant bad means every a :: bal(a) = r(a)\nend\n"
        );
        assert!(errors(&src).iter().any(|m| m.contains("cannot compare money(gbp) with a scalar")), "{:?}", errors(&src));
    }
}
