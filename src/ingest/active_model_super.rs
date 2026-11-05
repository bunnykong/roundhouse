// Not left as `super(**kwargs)`: past ActiveModel::Model's initialize there is only Object's, which takes no keywords, and that initialize is assigning each key through its writer.
use crate::app::App;
use crate::dialect::{MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub(super) fn rewrite(app: &mut App) {
    for model in &mut app.models {
        let includes_model = model.body.iter().any(|item| {
            matches!(item, ModelBodyItem::Unknown { expr, .. } if include_of(expr, &["ActiveModel::Model", "ActiveModel::API"]))
        });
        if !includes_model {
            continue;
        }
        let mut writers: Vec<String> = Vec::new();
        for item in &model.body {
            match item {
                ModelBodyItem::Unknown { expr, .. } => {
                    if let ExprNode::Send { recv: None, method, args, .. } = &*expr.node
                        && matches!(method.as_str(), "attr_accessor" | "attr_writer")
                    {
                        for a in args {
                            if let ExprNode::Lit { value: Literal::Sym { value } } = &*a.node {
                                writers.push(value.as_str().to_string());
                            }
                        }
                    }
                }
                ModelBodyItem::Method { method, .. } if method.receiver == MethodReceiver::Instance => {
                    if let Some(base) = method.name.as_str().strip_suffix('=')
                        && !base.is_empty()
                        && !matches!(base, "=" | "!" | "<" | ">" | "[]")
                    {
                        writers.push(base.to_string());
                    }
                }
                _ => {}
            }
        }
        writers.sort();
        writers.dedup();
        let class_name = model.name.0.as_str().to_string();
        for method in model.methods_mut() {
            if method.receiver != MethodReceiver::Instance || method.name.as_str() != "initialize" {
                continue;
            }
            let Some(rest) = method.params.iter().find(|p| p.keyword && p.rest).map(|p| p.name.clone()) else { continue };
            let Some(assign) = assignment(&rest, &writers, &class_name) else { continue };
            replace_super(&mut method.body, &rest, &assign);
        }
    }
}

fn include_of(e: &Expr, names: &[&str]) -> bool {
    let ExprNode::Send { recv: None, method, args, .. } = &*e.node else { return false };
    method.as_str() == "include"
        && args.iter().any(|a| matches!(&*a.node, ExprNode::Const { path }
            if names.contains(&path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::").as_str())))
}

fn assignment(rest: &Symbol, writers: &[String], class_name: &str) -> Option<Expr> {
    let r = rest.as_str();
    let mut src = format!("def __active_model_assign({r})\n  {r}.each do |key, value|\n    case key.to_s\n");
    for w in writers {
        src.push_str(&format!("    when \"{w}\" then self.{w} = value\n"));
    }
    src.push_str(&format!(
        "    else raise ActiveModel::UnknownAttributeError.new(self, key.to_s, \"unknown attribute '#{{key}}' for {class_name}.\")\n    end\n  end\n  nil\nend\n"
    ));
    let mut methods = crate::runtime_src::parse_methods(&src).ok()?;
    methods.pop().map(|m| m.body)
}

fn replace_super(e: &mut Expr, rest: &Symbol, assign: &Expr) {
    if let ExprNode::Super { args: Some(args) } = &*e.node
        && let [arg] = args.as_slice()
        && matches!(&*arg.node, ExprNode::KeywordSplat { value } if matches!(&*value.node, ExprNode::Var { name, .. } if name == rest))
    {
        let span = e.span;
        *e = assign.clone();
        e.span = span;
        return;
    }
    e.node.for_each_child_mut(&mut |c| replace_super(c, rest, assign));
}
