use quote::{ToTokens, quote};
use std::collections::HashSet;
use syn::visit::{Visit, visit_expr};
use syn::{Arm, Block, Expr, ExprMatch, Ident, Pat, Stmt};

/// A `match` arm of `fuse()` together with the two variants its pattern matches on
pub(super) struct FusedArm {
    /// Variant of the first (earlier) instruction of the pair
    pub(super) prev: Ident,
    /// Variant of the second (later) instruction of the pair
    pub(super) next: Ident,
    /// Variants the arm constructs, all of which must be present for the arm to be usable
    pub(super) constructed: HashSet<Ident>,
    pub(super) arm: Arm,
}

/// Whether an expression is exactly `(prev, next)`, which is what an implementation without any
/// fusions of its own consists of and what the fallback arm of every `match` is
fn is_unfused_pair(expr: &Expr) -> bool {
    expr.to_token_stream().to_string() == quote! { (prev, next) }.to_string()
}

/// Extracts the variant ident from an unqualified `Self::Variant` pattern in any of its forms
fn self_variant_ident(pat: &Pat) -> Option<&Ident> {
    let path = match pat {
        Pat::Struct(pat_struct) => &pat_struct.path,
        Pat::TupleStruct(pat_tuple_struct) => &pat_tuple_struct.path,
        Pat::Path(pat_path) => &pat_path.path,
        _ => {
            return None;
        }
    };

    if path.leading_colon.is_some() {
        return None;
    }

    let mut segments = path.segments.iter();
    match (segments.next(), segments.next(), segments.next()) {
        (Some(self_segment), Some(variant_segment), None)
            if self_segment.ident == "Self"
                && self_segment.arguments.is_empty()
                && variant_segment.arguments.is_empty() =>
        {
            Some(&variant_segment.ident)
        }
        _ => None,
    }
}

/// Collects every `Self::Variant` an expression constructs
struct ConstructedVariantsCollector {
    variants: HashSet<Ident>,
}

impl<'ast> Visit<'ast> for ConstructedVariantsCollector {
    fn visit_expr(&mut self, i: &'ast Expr) {
        let path = match i {
            Expr::Struct(expr_struct) => Some(&expr_struct.path),
            Expr::Path(expr_path) => Some(&expr_path.path),
            _ => None,
        };

        if let Some(path) = path
            && path.leading_colon.is_none()
        {
            let mut segments = path.segments.iter();
            if let (Some(self_segment), Some(variant_segment), None) =
                (segments.next(), segments.next(), segments.next())
                && self_segment.ident == "Self"
                && self_segment.arguments.is_empty()
                && variant_segment.arguments.is_empty()
            {
                self.variants.insert(variant_segment.ident.clone());
            }
        }

        visit_expr(self, i);
    }
}

fn constructed_variants(arm: &Arm) -> HashSet<Ident> {
    let mut collector = ConstructedVariantsCollector {
        variants: HashSet::new(),
    };
    collector.visit_expr(&arm.body);
    collector.variants
}

/// Turns a single `match` arm into [`FusedArm`], or `None` for the fallback arm, which is
/// re-created during composition rather than inherited
fn process_arm(arm: &Arm) -> anyhow::Result<Option<FusedArm>> {
    // TODO: A guard ends up as part of the pattern rather than in `Arm::guard` with the `syn`
    //  version in use, so it has to be looked through here
    let pat = match &arm.pat {
        Pat::Guard(pat_guard) => pat_guard.pat.as_ref(),
        pat => pat,
    };

    if matches!(pat, Pat::Wild(_)) {
        return Ok(None);
    }

    let Pat::Tuple(pat_tuple) = pat else {
        return Err(anyhow::anyhow!(
            "`match` pattern must be `(Self::Prev {{ .. }}, Self::Next {{ .. }})` or `_`: {}",
            arm.pat.to_token_stream()
        ));
    };

    let [prev, next] = pat_tuple.elems.iter().collect::<Vec<_>>()[..] else {
        return Err(anyhow::anyhow!(
            "`match` pattern must be a pair of instructions: {}",
            arm.pat.to_token_stream()
        ));
    };

    let (Some(prev), Some(next)) = (self_variant_ident(prev), self_variant_ident(next)) else {
        return Err(anyhow::anyhow!(
            "Both elements of a `match` pattern must be unqualified `Self::Variant`: {}",
            arm.pat.to_token_stream()
        ));
    };

    Ok(Some(FusedArm {
        prev: prev.clone(),
        next: next.clone(),
        constructed: constructed_variants(arm),
        arm: arm.clone(),
    }))
}

fn process_match(expr_match: &ExprMatch) -> anyhow::Result<Vec<FusedArm>> {
    if !is_unfused_pair(&expr_match.expr) {
        return Err(anyhow::anyhow!(
            "`match` must be on literal `(prev, next)`: {}",
            expr_match.expr.to_token_stream()
        ));
    }

    let mut fused_arms = Vec::with_capacity(expr_match.arms.len());
    for arm in &expr_match.arms {
        fused_arms.extend(process_arm(arm)?);
    }

    Ok(fused_arms)
}

/// Extracts the fusable pairs of `fuse()` method body, which must consist of a single `match` on
/// `(prev, next)` or of exactly `(prev, next)` when there is nothing to fuse
pub(super) fn extract_fused_arms(block: &Block) -> anyhow::Result<Vec<FusedArm>> {
    let mut stmts_iter = block.stmts.iter();

    let (Some(Stmt::Expr(expr, None)), None) = (stmts_iter.next(), stmts_iter.next()) else {
        return Err(anyhow::anyhow!(
            "Function body must be a single tail expression: {}",
            block.to_token_stream()
        ));
    };

    if is_unfused_pair(expr) {
        Ok(Vec::new())
    } else if let Expr::Match(expr_match) = expr {
        process_match(expr_match)
    } else {
        Err(anyhow::anyhow!(
            "Single tail expression must be either `match` on `(prev, next)` or `(prev, next)`: {}",
            expr.to_token_stream()
        ))
    }
}
