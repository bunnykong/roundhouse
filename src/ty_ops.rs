//! Operation-local DAG equality. No global interner, widening or normalization.
use crate::ty::Ty;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

/// a multiply-rotate hasher for pointer-pair memo keys
/// (SipHash dominated the memoized comparisons).
#[derive(Default, Clone, Copy)]
pub(crate) struct FxLike(u64);
impl std::hash::Hasher for FxLike {
    fn finish(&self) -> u64 { self.0 }
    fn write(&mut self, bytes: &[u8]) { for b in bytes { self.write_u64(*b as u64); } }
    fn write_u64(&mut self, x: u64) { self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95); }
    fn write_usize(&mut self, x: usize) { self.write_u64(x as u64); }
}
pub(crate) type PairHash = std::hash::BuildHasherDefault<FxLike>;
static STATS: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_ARC_STATS").is_ok_and(|s| s == "1"));
thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static MEMO: RefCell<HashMap<(usize,usize),bool,PairHash>> = RefCell::new(HashMap::default());
    static COUNTS: RefCell<Counts> = RefCell::new(Counts::default());
    static CMP_DEPTH: Cell<usize> = const { Cell::new(0) };
    static CMP_MEMO: RefCell<HashMap<(usize,usize),std::cmp::Ordering>> = RefCell::new(HashMap::new());
}
#[derive(Default, serde::Serialize)]
struct Counts { equality_calls: u64, equality_memo_hits: u64, equality_seconds: f64, witness_nodes: u64,
                witness_memo_hits: u64, substitution_nodes: u64, substitution_memo_hits: u64,
                ordering_calls: u64, ordering_memo_hits: u64 }
struct Scope(bool);
impl Drop for Scope {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get()-1));
        if self.0 { MEMO.with(|m| m.borrow_mut().clear()); }
    }
}
pub(crate) fn equal(a: &Ty, b: &Ty) -> bool {
    let root=DEPTH.with(|d| { let r=d.get()==0; d.set(d.get()+1); r });
    let _scope=Scope(root);
    let started=(root && *STATS).then(Instant::now);
    if *STATS { COUNTS.with(|c| c.borrow_mut().equality_calls+=1); }
    if std::ptr::eq(a,b) { return true; }
    // leaves and differing heads need no memo entry.
    if std::mem::discriminant(a)!=std::mem::discriminant(b) { return false; }
    match (a,b) {
        (Ty::Int,_)|(Ty::Float,_)|(Ty::Bool,_)|(Ty::Str,_)|(Ty::Sym,_)|(Ty::Date,_)|(Ty::Time,_)|(Ty::Nil,_)|
        (Ty::SelfInstance,_)|(Ty::Untyped,_)|(Ty::Bottom,_)=>return true,
        (Ty::Var{var:x},Ty::Var{var:y})=>return x==y,
        (Ty::Rec{slot:x},Ty::Rec{slot:y})=>return x==y,
        (Ty::Relation{of:x},Ty::Relation{of:y})=>return x==y,
        _=>{}
    }
    let key=(a as *const Ty as usize,b as *const Ty as usize);
    if let Some(value)=MEMO.with(|m| m.borrow().get(&key).copied()) {
        if *STATS { COUNTS.with(|c| c.borrow_mut().equality_memo_hits+=1); }
        return value;
    }
    let result=match (a,b) {
        (Ty::Int,Ty::Int)|(Ty::Float,Ty::Float)|(Ty::Bool,Ty::Bool)|(Ty::Str,Ty::Str)|
        (Ty::Sym,Ty::Sym)|(Ty::Date,Ty::Date)|(Ty::Time,Ty::Time)|(Ty::Nil,Ty::Nil)|
        (Ty::SelfInstance,Ty::SelfInstance)|(Ty::Untyped,Ty::Untyped)|(Ty::Bottom,Ty::Bottom)=>true,
        (Ty::Relation{of:x},Ty::Relation{of:y})=>x==y,
        (Ty::Array{elem:x},Ty::Array{elem:y})=>x==y,
        (Ty::Hash{key:kx,value:vx},Ty::Hash{key:ky,value:vy})=>kx==ky && vx==vy,
        (Ty::Tuple{elems:x},Ty::Tuple{elems:y})=>x==y,
        (Ty::Record{row:x},Ty::Record{row:y})=>x==y,
        (Ty::Union{variants:x},Ty::Union{variants:y})=>x==y,
        (Ty::Class{id:ix,args:ax},Ty::Class{id:iy,args:ay})=>ix==iy && ax==ay,
        (Ty::Fn{params:px,block:bx,ret:rx,effects:ex},Ty::Fn{params:py,block:by,ret:ry,effects:ey})=>
            px==py && bx==by && rx==ry && ex==ey,
        (Ty::Var{var:x},Ty::Var{var:y})=>x==y,
        // the fold's analysis-only reference.
        (Ty::Rec{slot:x},Ty::Rec{slot:y})=>x==y,
        _=>false,
    };
    MEMO.with(|m| { m.borrow_mut().insert(key,result); });
    if let Some(started)=started { COUNTS.with(|c| c.borrow_mut().equality_seconds+=started.elapsed().as_secs_f64()); }
    result
}
pub(crate) fn witness_visit(hit: bool) {
    if *STATS { COUNTS.with(|c| { let mut c=c.borrow_mut();
        if hit {c.witness_memo_hits+=1} else {c.witness_nodes+=1} }); }
}
pub(crate) fn substitution_visit(hit: bool) {
    if *STATS { COUNTS.with(|c| { let mut c=c.borrow_mut();
        if hit {c.substitution_memo_hits+=1} else {c.substitution_nodes+=1} }); }
}
struct CmpScope(bool);
impl Drop for CmpScope {
    fn drop(&mut self) {
        CMP_DEPTH.with(|d|d.set(d.get()-1));
        if self.0 { CMP_MEMO.with(|m|m.borrow_mut().clear()); }
    }
}
pub(crate) fn compare(a: &Ty, b: &Ty, compute: impl FnOnce()->std::cmp::Ordering)->std::cmp::Ordering {
    let root=CMP_DEPTH.with(|d|{let r=d.get()==0; d.set(d.get()+1);r});
    let _scope=CmpScope(root);
    if *STATS {COUNTS.with(|c|c.borrow_mut().ordering_calls+=1);}
    if std::ptr::eq(a,b) {return std::cmp::Ordering::Equal;}
    let key=(a as *const Ty as usize,b as *const Ty as usize);
    if let Some(result)=CMP_MEMO.with(|m|m.borrow().get(&key).copied()) {
        if *STATS {COUNTS.with(|c|c.borrow_mut().ordering_memo_hits+=1);}
        return result;
    }
    let result=compute();
    CMP_MEMO.with(|m|{m.borrow_mut().insert(key,result);});
    result
}
pub(crate) fn report() {
    if *STATS {
        COUNTS.with(|c| { let record=std::mem::take(&mut *c.borrow_mut());
            let mut record=serde_json::to_value(record).unwrap();
            let (calls,hits,entries,intern_calls,intern_hits,intern_entries)=crate::shared::cache_stats();
            record["shared_equal_calls"]=calls.into();
            record["shared_equal_hits"]=hits.into();
            record["shared_equal_entries"]=entries.into();
            record["intern_calls"]=intern_calls.into();
            record["intern_hits"]=intern_hits.into();
            record["intern_entries"]=intern_entries.into();
            let (hash_nodes,hash_hits)=crate::ty_hash::take_counts();
            let (join_calls,join_hits)=crate::join_memo::take_counts();
            record["hash_nodes"]=hash_nodes.into();record["hash_memo_hits"]=hash_hits.into();
            record["join_calls"]=join_calls.into();record["join_memo_hits"]=join_hits.into();
            eprintln!("rh-arc: {}",record); });
    }
}
