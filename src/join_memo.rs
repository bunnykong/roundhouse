//! Join keys own their inputs; wire equality protects record order.
use crate::ty::Ty;
use std::cell::{Cell,RefCell};
use std::collections::HashMap;
use std::hash::{Hash,Hasher};
#[derive(Clone)]
struct Wire(Ty);
impl PartialEq for Wire {fn eq(&self,other:&Self)->bool {crate::wire_ty::equal(&self.0,&other.0)}}
impl Eq for Wire {}
impl Hash for Wire {fn hash<H:Hasher>(&self,h:&mut H) {self.0.hash(h);}}
thread_local! {
    static DEPTH:Cell<usize>=const {Cell::new(0)};
    static MEMO:RefCell<HashMap<(Wire,Wire),Ty>>=RefCell::new(HashMap::new());
    static CALLS:Cell<u64>=const {Cell::new(0)};
    static HITS:Cell<u64>=const {Cell::new(0)};
}
struct Scope(bool);
impl Drop for Scope {fn drop(&mut self) {
    DEPTH.with(|d|d.set(d.get()-1));
    if self.0 {MEMO.with(|m|m.borrow_mut().clear());}
}}
pub(crate) fn join(a:Ty,b:Ty,compute:impl FnOnce(Ty,Ty)->Ty)->Ty {
    let root=DEPTH.with(|d|{let root=d.get()==0;d.set(d.get()+1);root});
    let _scope=Scope(root);
    CALLS.with(|n|n.set(n.get()+1));
    let key=(Wire(a.clone()),Wire(b.clone()));
    if let Some(value)=MEMO.with(|m|m.borrow().get(&key).cloned()) {
        HITS.with(|n|n.set(n.get()+1));return value;
    }
    let value=compute(a,b);
    MEMO.with(|m|{m.borrow_mut().insert(key,value.clone());});value
}
pub(crate) fn take_counts()->(u64,u64) {(CALLS.with(|n|n.replace(0)),HITS.with(|n|n.replace(0)))}
/// the root join memo is off unless `RH_C2_JOINMEMO=1`
/// (sharing's 1g default); with the fold, roots are many small joins and
/// the per-root memo only cost hashing and copies.
pub(crate) fn enabled()->bool {
    static ON:std::sync::LazyLock<bool>=std::sync::LazyLock::new(||std::env::var("RH_C2_JOINMEMO").is_ok_and(|v|v=="1"));
    *ON
}
