// Not left on the receiver: `permit`, `permit!`, `to_unsafe_h` and `require` are ActionController::Parameters methods, and a request's params reach the emitted program as plain String-keyed hashes.
use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_params_permit_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn is_param_value(ty: Option<&Ty>) -> bool {
    matches!(ty, Some(Ty::Class { id, .. }) if id.0.as_str() == crate::analyze::PARAM_VALUE)
}

/// Rails' `ActionController::Parameters`, which a gem's code names (Devise's `ParameterSanitizer`), or a value read
/// out of one: on the emitted program both are the request's String-keyed Hash.
fn is_parameters(ty: Option<&Ty>) -> bool {
    match ty {
        Some(Ty::Class { id, .. }) => id.0.as_str() == "ActionController::Parameters" || id.0.as_str() == crate::analyze::PARAM_VALUE,
        Some(Ty::Union { variants }) => variants.iter().all(|v| *v == Ty::Nil || is_parameters(Some(v))),
        _ => false,
    }
}

/// Parameters beside another Hash (Devise's `parse_authentication_key_values(hash, keys)`, handed a request's
/// params and a Symbol-keyed Hash): which one a read meets is the run time's.
/// Parameters among other values: only Parameters answer `permit`.
fn holds_parameters(ty: Option<&Ty>) -> bool {
    matches!(ty, Some(Ty::Union { variants }) if variants.iter().any(|v| is_parameters(Some(v))))
}

fn may_be_parameters(ty: Option<&Ty>) -> bool {
    matches!(ty, Some(Ty::Union { variants }) if variants.iter().any(|v| is_parameters(Some(v)))
        && variants.iter().all(|v| is_parameters(Some(v)) || matches!(v, Ty::Hash { .. } | Ty::Nil)))
}

/// A request's own params, read through `params` (Warden's strategies' `request.params`), which Rails reads by a
/// Symbol as by its String. Not any `Hash[String, …]` a variable holds: a permitted Symbol-keyed Hash reaches those too.
fn is_request_params_read(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { method, args, .. } if method.as_str() == "params" && args.is_empty())
        && matches!(e.ty.as_ref(), Some(Ty::Hash { key, value }) if **key == Ty::Str && is_param_value(Some(value)))
}

fn is_request_params(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Send { recv: None, method, args, .. } => method.as_str() == "params" && args.is_empty(),
        ExprNode::Ivar { name } => name.as_str() == "params",
        _ => false,
    }
}

/// Array methods whose block's first parameter is one element.
const ELEMENT_BLOCK_METHODS: &[&str] =
    &["map", "collect", "each", "flat_map", "filter_map", "select", "filter", "reject", "each_with_index"];

/// `params.require(:list)`, before or after it lowers to `Params.require_key`.
fn is_required_list(e: &Expr) -> bool {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node else { return false };
    match method.as_str() {
        "require" => is_request_params(r) && args.len() == 1,
        "require_key" => {
            matches!(&*r.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Params")
                && args.first().is_some_and(is_request_params)
        }
        _ => false,
    }
}

/// `params.require(:list).map { |item| ... }`: Rails makes each element an
/// `ActionController::Parameters` (actionpack 8.1.4), and the runtime's
/// are String-keyed Hashes, so the element's own calls lower here.
fn rewrite_element_block(expr: &mut Expr) {
    let ExprNode::Send { recv: Some(r), method, block: Some(block), .. } = &mut *expr.node else { return };
    if !ELEMENT_BLOCK_METHODS.contains(&method.as_str()) || !is_required_list(r) {
        return;
    }
    let ExprNode::Lambda { params, body, .. } = &mut *block.node else { return };
    let Some(item) = params.first().cloned() else { return };
    rewrite_element_calls(body, &item);
}

fn rewrite_element_calls(expr: &mut Expr, item: &Symbol) {
    if matches!(&*expr.node, ExprNode::Lambda { params, .. } if params.contains(item)) {
        return;
    }
    expr.node.for_each_child_mut(&mut |c| rewrite_element_calls(c, item));
    let ExprNode::Send { recv: Some(r), method, args, block, .. } = &mut *expr.node else { return };
    if !matches!(&*r.node, ExprNode::Var { name, .. } if name == item) {
        return;
    }
    match method.as_str() {
        "permit" | "expect" if block.is_none() => {
            if let Some(mut new) = element_filter(expr) {
                new.ty = expr.ty.clone();
                *expr = new;
            }
        }
        "[]" | "fetch" | "key?" | "has_key?" | "include?" | "member?" | "dig" => {
            let keys = if method.as_str() == "dig" { args.len() } else { args.len().min(1) };
            for key in args.iter_mut().take(keys) {
                if let Some(name) = sym_name(key) {
                    *key = str_lit(key.span, name);
                }
            }
        }
        _ => {}
    }
}

/// An element's `permit(:a, b: [...])`, and `expect(:a)`, which is Rails'
/// permit then require: a nested or blank value raises `ParameterMissing`.
fn element_filter(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node else { return None };
    let params_const = || Expr::new(e.span, ExprNode::Const { path: vec![Symbol::from("Params")] });
    let allowed = |names: Vec<String>| {
        let mut list = Expr::new(e.span, ExprNode::Array {
            elements: names.into_iter().map(|n| str_lit(e.span, n)).collect(),
            style: Default::default(),
        });
        list.ty = Some(Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
        send(e.span, Some(params_const()), "allowed_scalars", vec![r.clone(), list])
    };
    if method.as_str() == "expect" {
        let [key] = args.as_slice() else { return None };
        let name = sym_name(key)?;
        return Some(send(e.span, Some(params_const()), "require_key", vec![allowed(vec![name.clone()]), str_lit(e.span, name)]));
    }
    if matches!(args.last().map(|a| &*a.node), Some(ExprNode::Hash { .. })) {
        return filtered_call(e);
    }
    if args.is_empty() {
        return None;
    }
    Some(allowed(args.iter().map(sym_name).collect::<Option<Vec<_>>>()?))
}

fn rewrite(expr: &mut Expr) {
    rewrite_element_block(expr);
    expr.node.for_each_child_mut(&mut rewrite);
    // A top-level `params.permit(:a, :b)` CONSUMED IN PLACE, as a call's
    // argument — the Rails 8 authentication generator's
    // `@user.update(params.permit(:password, :password_confirmation))`
    // and `User.authenticate_by(params.permit(:email_address,
    // :password))`. One RETURNED from a params helper (`def note_params
    // = params.permit(:body)`) is the controller lowering's: it becomes
    // that controller's typed params class.
    if let ExprNode::Send { args, .. } = &mut *expr.node {
        for arg in args.iter_mut() {
            if let Some(permitted) = permitted_chain(arg) {
                *arg = permitted;
            }
        }
    }
    if parameters_on_hash(expr) {
        return;
    }
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node else { return };
    let nested = is_param_value(r.ty.as_ref());
    let replacement = match (method.as_str(), args.as_slice()) {
        ("permit!" | "to_unsafe_h", []) if nested || is_request_params(r) => Some(r.clone()),
        ("permit", keys) if nested && !keys.is_empty() => {
            let Some(names) = keys.iter().map(sym_name).collect::<Option<Vec<_>>>() else { return };
            Some(send(expr.span, Some(r.clone()), "slice", names.into_iter().map(|n| str_lit(expr.span, n)).collect()))
        }
        ("require", [key]) if nested => {
            let Some(name) = sym_name(key) else { return };
            let params = Expr::new(expr.span, ExprNode::Const { path: vec![Symbol::from("Params")] });
            Some(send(expr.span, Some(params), "require_key", vec![r.clone(), str_lit(expr.span, name)]))
        }
        _ => None,
    };
    if let Some(mut new) = replacement {
        new.ty = expr.ty.clone();
        *expr = new;
    }
}

/// Every top-level `params.permit(:a, :b)` left in a controller body, in
/// any position: a params helper's tail that names no resource
/// (lobsters' `def search_params = params.permit(:q, :what, …)`) is not
/// a typed params class's, and emitted verbatim it called `permit` on a
/// Hash.
pub(crate) fn rewrite_bare_permits(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_bare_permits);
    if let Some(mut permitted) = permitted_chain(expr) {
        permitted.ty = Some(permitted_ty());
        *expr = permitted;
    } else if let Some(filtered) = filtered_chain(expr) {
        *expr = filtered;
    }
}

/// The private helpers whose value is a nested `params.permit` (`def
/// moderation_params = @moderation_params ||= params.permit(:page, what:
/// [...])`), so their call sites can carry the String-keyed type.
pub(crate) fn filtered_permit_helpers(privs: &[crate::dialect::Action]) -> crate::det::HashSet<Symbol> {
    fn tail(e: &Expr) -> &Expr {
        match &*e.node {
            ExprNode::Seq { exprs } if !exprs.is_empty() => tail(exprs.last().unwrap()),
            ExprNode::OpAssign { value, .. } | ExprNode::Assign { value, .. } => tail(value),
            _ => e,
        }
    }
    privs
        .iter()
        .filter(|a| a.params.fields.is_empty() && filtered_chain(tail(&a.body)).is_some())
        .map(|a| a.name.clone())
        .collect()
}

/// `moderation_params.dig(:what, :stories)` → `.dig("what", "stories")`,
/// the indifferent read Rails' Parameters gives on String-keyed storage.
/// Not left to the Ruby emit's typed key coercion: these helper calls reach it untyped.
pub(crate) fn stringify_filtered_helper_reads(expr: &mut Expr, helpers: &crate::det::HashSet<Symbol>) {
    expr.node.for_each_child_mut(&mut |c| stringify_filtered_helper_reads(c, helpers));
    let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *expr.node else { return };
    let helper_call = matches!(&*r.node, ExprNode::Send { recv, method, args, block: None, .. }
        if args.is_empty()
            && helpers.contains(method)
            && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)));
    if !helper_call {
        return;
    }
    let keys = match method.as_str() {
        "dig" => args.len(),
        "[]" | "fetch" | "key?" | "has_key?" | "include?" | "member?" => args.len().min(1),
        _ => 0,
    };
    for key in args.iter_mut().take(keys) {
        if let Some(name) = sym_name(key) {
            *key = str_lit(key.span, name);
        }
    }
}

/// `params.permit(:a, what: [:x, :y])` → `Params.filtered_values(params,
/// ["a"], {"what" => ["x", "y"]}, {"what" => "any"})`.
/// Not the Symbol-keyed `permitted` hash: a nested filter's value is a
/// hash or list, which a `Hash[Symbol, String]` cannot hold, so this keeps
/// Rails' own String-keyed storage and the emit's indifferent key reads.
fn filtered_chain(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: Some(r), method, block: None, .. } = &*e.node else { return None };
    if method.as_str() != "permit" || !is_request_params(r) || is_param_value(r.ty.as_ref()) {
        return None;
    }
    filtered_call(e)
}

/// `Params.filtered_values` for a `permit` call whose last argument is a
/// Hash of nested filters.
fn filtered_call(e: &Expr) -> Option<Expr> {
    use crate::lower::controller_to_library::params::{nested_candidates, params_hash_ty};
    let ExprNode::Send { recv: Some(r), args, .. } = &*e.node else { return None };
    let (last, scalars) = args.split_last()?;
    let ExprNode::Hash { entries, .. } = &*last.node else { return None };
    let names = scalars.iter().map(sym_name).collect::<Option<Vec<_>>>()?;
    let nested = nested_candidates(e, &crate::ident::ClassId(Symbol::from("")));
    // A key the parser could not read (`k: [{ … }]`) would be dropped silently.
    if entries.is_empty() || nested.len() != entries.len() {
        return None;
    }
    let span = e.span;
    let str_array = |items: Vec<String>| {
        let mut a = Expr::new(span, ExprNode::Array {
            elements: items.into_iter().map(|s| str_lit(span, s)).collect(),
            style: Default::default(),
        });
        a.ty = Some(Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
        a
    };
    let hash = |entries: Vec<(Expr, Expr)>, value: Ty| {
        let mut h = Expr::new(span, ExprNode::Hash { entries, kwargs: false });
        h.ty = Some(Ty::Hash { key: std::sync::Arc::new(Ty::Str), value: std::sync::Arc::new(value) });
        h
    };
    let children = hash(
        nested.iter().map(|n| (str_lit(span, n.name.as_str().to_string()), str_array(n.allowed.clone()))).collect(),
        Ty::Array { elem: std::sync::Arc::new(Ty::Str) },
    );
    let modes = hash(
        nested.iter().map(|n| (str_lit(span, n.name.as_str().to_string()), str_lit(span, n.mode.to_string()))).collect(),
        Ty::Str,
    );
    let params_const = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("Params")] });
    let mut out = send(span, Some(params_const), "filtered_values", vec![r.clone(), str_array(names), children, modes]);
    out.ty = Some(params_hash_ty());
    Some(out)
}

/// `params.permit(:a, :b)` on the request's params →
/// `Params.permitted(Params.permitted({}, params, "a", :a), params, "b", :b)`.
/// A model write reads Symbol keys, so this builds a Symbol-keyed hash
/// of the scalars the request provided, where `slice` would have handed
/// it String keys it never reads.
fn permitted_chain(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node else { return None };
    if method.as_str() != "permit" || args.is_empty() || !is_request_params(r) || is_param_value(r.ty.as_ref()) {
        return None;
    }
    let names = args.iter().map(sym_name).collect::<Option<Vec<_>>>()?;
    let params_const = || Expr::new(e.span, ExprNode::Const { path: vec![Symbol::from("Params")] });
    let mut acc = Expr::new(e.span, ExprNode::Hash { entries: vec![], kwargs: false });
    acc.ty = Some(permitted_ty());
    for name in names {
        let sym = Expr::new(e.span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name.as_str()) } });
        acc = send(e.span, Some(params_const()), "permitted", vec![acc, r.clone(), str_lit(e.span, name), sym]);
        acc.ty = Some(permitted_ty());
    }
    Some(acc)
}

/// What `Params.permitted` answers: the Symbol-keyed scalars.
fn permitted_ty() -> Ty {
    Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(Ty::Str) }
}

fn sym_name(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    }
}

fn str_lit(span: crate::span::Span, value: String) -> Expr {
    let mut e = Expr::new(span, ExprNode::Lit { value: Literal::Str { value } });
    e.ty = Some(Ty::Str);
    e
}

fn send(span: crate::span::Span, recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true },
    )
}

/// Rails' `Parameters` API as a gem writes it, on the Hash the emitted program holds:
/// `Parameters.new(hash)` is the hash, a nested Parameters is a Hash (`respond_to?(:permit)`), a key a Symbol
/// names is its String (`@params[@resource_name]`), and `permit(*names)` keeps the scalars under those names.
fn parameters_on_hash(expr: &mut Expr) -> bool {
    let span = expr.span;
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *expr.node else { return false };
    let is_parameters_const = matches!(&*r.node, ExprNode::Const { path }
        if path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>() == ["ActionController", "Parameters"]);
    // Not a fresh Request: Rails' wraps the env, whose state is the request the routes serve (Devise's warden_compat).
    let is_request_const = matches!(&*r.node, ExprNode::Const { path }
        if path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>() == ["ActionDispatch", "Request"]);
    if is_request_const && method.as_str() == "new" && args.len() == 1 {
        *method = Symbol::from("for_env");
        return true;
    }
    if is_parameters_const && method.as_str() == "new" {
        if let [hash] = args.as_slice() {
            let mut hash = hash.clone();
            hash.ty = expr.ty.clone();
            *expr = hash;
            return true;
        }
        return false;
    }
    if method.as_str() == "respond_to?" && holds_parameters(r.ty.as_ref()) && let [name] = args.as_slice() && sym_name(name).as_deref() == Some("permit") {
        let hash = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("Hash")] });
        let mut test = send(span, Some(r.clone()), "is_a?", vec![hash]);
        test.ty = Some(Ty::Bool);
        *expr = test;
        return true;
    }
    // `permit(*names)` on a value that may be Parameters (Devise's `permit_keys(parameters, keys)`): only Parameters answers it.
    if method.as_str() == "permit"
        && holds_parameters(r.ty.as_ref())
        && matches!(&*r.node, ExprNode::Var { .. } | ExprNode::Ivar { .. })
        && let [arg] = args.as_slice()
        && let ExprNode::Splat { value } = &*arg.node
    {
        let names = value.clone();
        let params = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("Params")] });
        let ty = expr.ty.clone();
        *expr = send(span, Some(params), "permitted_names", vec![r.clone(), names]);
        expr.ty = ty;
        return true;
    }
    // Not a String read of a value that may be either: Parameters hold String keys here, the Hash beside them Symbols.
    let value_holder = !matches!(&*r.node, ExprNode::Send { method, .. } if method.as_str() == "params");
    // A Symbol read of a request value that may be Parameters among scalars (Devise's `attributes[:reset_password_token]`
    // over `params.fetch(resource_name, {})`): only Parameters answer it, a String or an Array raising as in Rails.
    let symbol_read = method.as_str() == "[]" && matches!(args.as_slice(), [k] if matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { .. } }));
    if value_holder && (may_be_parameters(r.ty.as_ref()) || (symbol_read && holds_parameters(r.ty.as_ref()))) && !is_parameters(r.ty.as_ref()) {
        // Not a literal String key, which reads alike either way.
        let symbol_key = |k: &Expr| !matches!(&*k.node, ExprNode::Lit { value: Literal::Str { .. } }) && !matches!(k.ty, Some(Ty::Str | Ty::Int));
        if method.as_str() == "[]" && let [key] = args.as_slice() && symbol_key(key) {
            let params = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("Params")] });
            let ty = expr.ty.clone();
            *expr = send(span, Some(params), "indifferent_read", vec![r.clone(), key.clone()]);
            expr.ty = ty;
            return true;
        }
        return false;
    }
    if !is_parameters(r.ty.as_ref()) && !is_request_params_read(r) {
        return false;
    }
    match (method.as_str(), args.as_mut_slice()) {
        ("respond_to?", [name]) if sym_name(name).as_deref() == Some("permit") => {
            let hash = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("Hash")] });
            let mut test = send(span, Some(r.clone()), "is_a?", vec![hash]);
            test.ty = Some(Ty::Bool);
            *expr = test;
            true
        }
        // Not a literal key: the emit's coercion writes those as Rails reads them.
        ("[]" | "fetch" | "key?" | "has_key?", [key, ..])
            if !matches!(key.ty, Some(Ty::Str)) && !matches!(&*key.node, ExprNode::Lit { .. }) =>
        {
            let sym = key.clone();
            let mut text = send(span, Some(sym), "to_s", vec![]);
            text.ty = Some(Ty::Str);
            *key = text;
            false
        }
        // Not a request's `params.require(:x).permit(*names)`: the controller lowering types that one.
        ("permit", [arg]) if let ExprNode::Splat { value } = &*arg.node && matches!(&*r.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) => {
            let names = value.clone();
            let params = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("Params")] });
            let ty = expr.ty.clone();
            *expr = send(span, Some(params), "permitted_names", vec![r.clone(), names]);
            expr.ty = ty;
            true
        }
        _ => false,
    }
}
