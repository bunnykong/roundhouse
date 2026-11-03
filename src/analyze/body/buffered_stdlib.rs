//! Refuse the intermediate-output forms the bundled one-shot zlib cannot serve.
use super::{BodyTyper, ClassId, Expr, ExprNode, Literal, ResolvedConstant, Symbol, Ty};

impl BodyTyper<'_> {
    pub(super) fn unsupported_zlib_stream_result(
        &self, ty: &Ty, method: &Symbol, args: &[Expr], has_block: bool,
    ) -> Option<&'static str> {
        if let Ty::Union { variants } = ty {
            return variants.iter().find_map(|ty| self.unsupported_zlib_stream_result(ty, method, args, has_block));
        }
        let Ty::Class { id, .. } = ty else { return None };
        let mut pending: Vec<&ClassId> = vec![id];
        let mut seen = crate::det::HashSet::default();
        let mut owner = None;
        let mut delegates = false;
        while let Some(id) = pending.pop() {
            if !seen.insert(id) { continue; }
            let Some(info) = self.classes().get(id) else { continue };
            if info.source_instance_methods.contains(method) {
                if !info.delegating_instance_methods.contains(method) { return None; }
                delegates = true;
            }
            if matches!(id.0.as_str(), "Zlib::Deflate" | "Zlib::GzipReader") {
                owner = Some(id.0.as_str());
                break;
            }
            pending.extend(info.parent.iter());
            pending.extend(info.includes.iter());
        }
        let owner = owner?;
        if delegates && matches!((owner, method.as_str()), ("Zlib::Deflate", "deflate") | ("Zlib::GzipReader", "read")) {
            return Some("zlib overrides forwarding through super are not qualified for buffered results");
        }
        if matches!(method.as_str(), "send" | "__send__" | "public_send" | "try" | "try!" | "method" | "public_method") {
            let (name, rest) = args.split_first()?;
            let name = match &*name.node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.clone(),
                ExprNode::Lit { value: Literal::Str { value } } => Symbol::from(value.as_str()),
                _ => return Some("computed calls on buffered zlib objects are not qualified"),
            };
            let captured = matches!(method.as_str(), "method" | "public_method");
            return self.unsupported_zlib_stream_result(ty, &name, rest, has_block || captured);
        }
        match (owner, method.as_str()) {
            ("Zlib::Deflate", "deflate") => {
                let finish = args.len() == 2 && args.get(1).is_some_and(|arg| {
                    if matches!(&*arg.node, ExprNode::Lit { value: Literal::Int { value: 4 } }) { return true; }
                    let ExprNode::Const { path } = &*arg.node else { return false };
                    matches!(self.const_resolver.as_ref().and_then(|r| r.reference(arg.span, path)),
                        Some(Some(ResolvedConstant::Value { name, runtime: Some(_), .. })) if name.0.as_str() == "Zlib::FINISH")
                });
                (!finish || has_block).then_some("Zlib::Deflate#deflate requires an explicit Zlib::FINISH (or literal 4) and no block; intermediate output differs on Spinel")
            }
            ("Zlib::GzipReader", "read") => (!args.is_empty() || has_block).then_some(
                "Zlib::GzipReader#read is qualified only without arguments or a block; length-limited reads differ at EOF on Spinel"),
            _ => None,
        }
    }
}
