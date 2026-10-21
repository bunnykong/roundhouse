use super::*;

fn source(ruby: &str) -> App {
    crate::ingest::survey::activate();
    crate::ingest::app::ingest_app_from_tree(HashMap::from([
        (std::path::PathBuf::from("lib/return_owners.rb"), ruby.as_bytes().to_vec()),
    ])).unwrap()
}

fn id(name: &str) -> ClassId { ClassId(Symbol::from(name)) }

#[test]
#[ignore = "RH_OWN_RETURNS=1"]
fn own_returns_syntax_beats_copy_history_on_both_sides() {
    let app = source(r#"
module ReturnMixin
  def export_data; nil; end
  def borrowed; nil; end
end
class ReturnOwner
  include ReturnMixin
  def export_data; "own CSV"; end
  def self.export_data; "own class CSV"; end
end
"#);
    let mut analyzer = Analyzer::new(&app);
    let owner = id("ReturnOwner");
    let mixin = id("ReturnMixin");
    let name = Symbol::from("export_data");
    let borrowed = Symbol::from("borrowed");
    assert!(analyzer.own_return(&owner, &name, MethodReceiver::Instance));
    assert!(analyzer.own_return(&owner, &name, MethodReceiver::Class));
    let module = analyzer.classes.get_mut(&mixin).unwrap();
    module.instance_methods.insert(name.clone(), Ty::Bottom);
    module.class_methods.insert(name.clone(), Ty::Bottom);
    module.instance_methods.insert(borrowed.clone(), Ty::Int);
    let host = analyzer.classes.get_mut(&owner).unwrap();
    host.instance_methods.insert(name.clone(), Ty::Str);
    host.class_methods.insert(name.clone(), Ty::Str);
    // Reproduce a previous copy's ownership mark, then a real own harvest.
    analyzer.concern_folded.insert(owner.clone(),
        ([name.clone()].into_iter().collect(), [name.clone()].into_iter().collect()));
    analyzer.fold_concern_surfaces(&app);
    assert_eq!(analyzer.classes[&owner].instance_methods[&name], Ty::Str);
    assert_eq!(analyzer.classes[&owner].class_methods[&name], Ty::Str);
    assert_eq!(analyzer.classes[&owner].instance_methods[&borrowed], Ty::Int);
    analyzer.classes.get_mut(&mixin).unwrap().instance_methods.insert(borrowed.clone(), Ty::Bool);
    analyzer.fold_concern_surfaces(&app);
    assert_eq!(analyzer.classes[&owner].instance_methods[&borrowed], Ty::Bool,
        "a copied return still follows its current module writer");
}

#[test]
#[ignore = "RH_OWN_RETURNS=1"]
fn own_returns_extend_protects_only_the_declared_receiver_side() {
    let app = source(r#"
module ExtendedReturnMixin
  def export_data; nil; end
  def instance_only; nil; end
end
class ExtendedReturnOwner
  extend ExtendedReturnMixin
  def self.export_data; "own CSV"; end
  def instance_only; "instance"; end
end
"#);
    let mut analyzer = Analyzer::new(&app);
    let owner = id("ExtendedReturnOwner");
    let mixin = id("ExtendedReturnMixin");
    let name = Symbol::from("export_data");
    let instance = Symbol::from("instance_only");
    assert!(analyzer.own_return(&owner, &name, MethodReceiver::Class));
    assert!(!analyzer.own_return(&owner, &instance, MethodReceiver::Class));
    let module = analyzer.classes.get_mut(&mixin).unwrap();
    module.instance_methods.insert(name.clone(), Ty::Bottom);
    module.instance_methods.insert(instance.clone(), Ty::Int);
    analyzer.classes.get_mut(&owner).unwrap().class_methods.insert(name.clone(), Ty::Str);
    analyzer.concern_folded.entry(owner.clone()).or_default().1.insert(name.clone());
    analyzer.fold_extended_modules(&app);
    assert_eq!(analyzer.classes[&owner].class_methods[&name], Ty::Str);
    assert_eq!(analyzer.classes[&owner].class_methods[&instance], Ty::Int);
}
