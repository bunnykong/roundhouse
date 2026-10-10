//! Experimental, chronological evaluation replay for `check` only.
//!
//! No final analyzer state is loaded. The ordinary driver starts with its
//! freshly constructed registry and replays surviving evaluations in trace
//! order. Equality is the conservative first covered-read test. Uncovered
//! evaluations run normally and the existing worklist finishes the phase.
//! The staged transfers are not all monotone: a cold shadow is mandatory.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::body::{ClassInfo, Ctx};
use super::sccq::{Family, Unit};
use crate::App;
use crate::expr::Expr;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

mod fingerprint;
mod profile;
mod wire;
pub(crate) use profile::{Phase, phase};
use fingerprint::{class_hash, inputs};
use wire::{SiteMap, encode};

pub(crate) static ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    unit: String,
    body: u64,
    root: u32,
    input: u64,
    reads: BTreeMap<String, u64>,
    write: Value,
    fold_writes: Vec<(Value, Value)>,
    scheduler: Option<SchedulerReads>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SchedulerReads {
    // None is S3's unindexed-class dependency (index 0).
    classes: Vec<Option<String>>,
    wild: Vec<Option<String>>,
    const_ids: Vec<u64>,
    const_names: Vec<String>,
    fold_slots: Vec<Value>,
}

impl SchedulerReads {
    fn capture(reads: super::sccq::Reads) -> Option<Self> {
        SESSION.with(|s| {
            let s = s.borrow();
            let by_idx = &s.as_ref()?.classes_by_idx;
            let names = |indices: &[u32]| -> Option<Vec<Option<String>>> {
                indices
                    .iter()
                    .map(|i| {
                        if *i == 0 {
                            Some(None)
                        } else {
                            by_idx.get(i).cloned().map(Some)
                        }
                    })
                    .collect()
            };
            Some(Self {
                classes: names(&reads.classes)?,
                wild: names(&reads.wild)?,
                const_ids: reads.const_ids.iter().map(|id| id.get()).collect(),
                const_names: reads
                    .const_names
                    .iter()
                    .map(|n| n.as_str().to_owned())
                    .collect(),
                fold_slots: reads
                    .fold_slots
                    .iter()
                    .map(|id| super::fold::key_of(*id).and_then(|key| wire::slot_key(&key)))
                    .collect::<Option<Vec<_>>>()?,
            })
        })
    }

    fn ready(&self, classes: &HashMap<ClassId, ClassInfo>) -> bool {
        self.classes.iter().chain(&self.wild).all(|name| {
            name.as_ref()
                .is_none_or(|name| classes.contains_key(&ClassId(Symbol::from(name.as_str()))))
        }) && self.const_ids.iter().all(|id| *id != 0)
            && self
                .fold_slots
                .iter()
                .all(|key| wire::decode_key(key).is_some())
    }

    fn replay(&self, classes: &HashMap<ClassId, ClassInfo>) {
        let index = |name: &Option<String>| {
            name.as_ref().map_or(0, |name| {
                classes[&ClassId(Symbol::from(name.as_str()))].sccq_idx
            })
        };
        for name in &self.classes {
            super::sccq::rec_class(index(name));
        }
        for name in &self.wild {
            super::sccq::rec_wild(index(name));
        }
        for id in &self.const_ids {
            super::sccq::rec_const_id(&rubydex::model::ids::DeclarationId::new(*id));
        }
        for name in &self.const_names {
            super::sccq::rec_const_name(&Symbol::from(name.as_str()));
        }
        for key in &self.fold_slots {
            super::sccq::rec_fold_slot(super::fold::warm_intern(wire::decode_key(key).unwrap()));
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    schema: u32,
    compiler: String,
    flags: Vec<(String, String)>,
    environment: u64,
    records: Vec<Record>,
}

#[derive(Default, Serialize)]
pub(crate) struct Stats {
    loaded: usize,
    invalidated: usize,
    replayed: usize,
    uncovered: usize,
    typed: usize,
    unused: usize,
    recorded: usize,
}

struct Session {
    dir: PathBuf,
    cache: Cache,
    old: Vec<Record>,
    cursor: usize,
    units: HashMap<u32, (String, u64, Vec<Symbol>)>,
    unit: Option<u32>,
    root: u32,
    depth: usize,
    current: Option<Record>,
    sites: SiteMap,
    stats: Stats,
    classes_by_idx: HashMap<u32, String>,
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

fn flags() -> Vec<(String, String)> {
    let mut flags: Vec<_> = std::env::vars()
        .filter(|(k, _)| {
            k.starts_with("RH_") && !k.starts_with("RH_WARM") && !k.starts_with("RH_FIXPOINT")
        })
        .collect();
    flags.sort_unstable();
    flags
}

fn decode_cache(bytes: &[u8]) -> serde_json::Result<Cache> {
    // A valid ingested expression can nest beyond JSON's default depth
    // limit. Only the owned trace reader opts out of that parser limit.
    let mut reader = serde_json::Deserializer::from_slice(bytes);
    reader.disable_recursion_limit();
    let cache = Cache::deserialize(&mut reader)?;
    reader.end()?;
    Ok(cache)
}

/// Called by the check command, never by emission or the LSP.
pub(crate) fn start(app: &App) -> Result<bool, String> {
    let Some(dir) = std::env::var_os("RH_WARM").filter(|d| !d.is_empty()) else {
        return Ok(false);
    };
    for (key, value) in [
        ("RH_SCHED", "sccq"),
        ("RH_BRK_ALLARMS", "1"),
        ("RH_FOLD_JOIN", "1"),
        ("RH_FOLD", "1"),
        ("RH_FOLD_SLOTS", "1"),
        ("RH_FOLD_TAIL", "1"),
        ("RH_WARM_SHADOW", "1"),
    ] {
        if std::env::var(key).as_deref() != Ok(value) {
            return Err(format!("RH_WARM requires {key}={value}"));
        }
    }
    let dir = PathBuf::from(dir);
    profile::start();
    let environment = fingerprint::environment(app);
    let cache = Cache {
        schema: 2,
        compiler: crate::version::COMMIT.unwrap_or("unknown").to_owned(),
        flags: flags(),
        environment,
        records: Vec::new(),
    };
    let load = phase(Phase::Load);
    let old = match std::fs::read(dir.join("evaluations.json")) {
        Ok(bytes) => match decode_cache(&bytes) {
            Ok(c)
                if c.schema == cache.schema
                    && c.compiler == cache.compiler
                    && c.flags == cache.flags
                    && c.environment == environment =>
            {
                c.records
            }
            _ => {
                eprintln!("rh-warm: cache incompatible; starting cold");
                Vec::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("read warm cache: {e}")),
    };
    drop(load);
    let loaded = old.len();
    SESSION.with(|s| {
        *s.borrow_mut() = Some(Session {
            dir,
            cache,
            old,
            cursor: 0,
            units: HashMap::new(),
            unit: None,
            root: 0,
            depth: 0,
            current: None,
            sites: SiteMap::new(app),
            classes_by_idx: HashMap::new(),
            stats: Stats {
                loaded,
                ..Stats::default()
            },
        })
    });
    ACTIVE.store(true, Ordering::Relaxed);
    Ok(true)
}

pub(crate) fn read(key: String, value: u64) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    SESSION.with(|s| {
        if let Some(s) = s.borrow_mut().as_mut() {
            if let Some(r) = s.current.as_mut() {
                r.reads.entry(key).or_insert(value);
            }
        }
    });
}

pub(crate) fn names() -> Vec<Symbol> {
    SESSION.with(|s| {
        let s = s.borrow();
        s.as_ref()
            .and_then(|s| s.unit.and_then(|u| s.units.get(&u)))
            .map(|(_, _, n)| n.clone())
            .unwrap_or_default()
    })
}

pub(crate) fn read_class(id: &ClassId, info: Option<&ClassInfo>, wild: bool) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let key = format!("{}:{}", if wild { "wild" } else { "class" }, id.0.as_str());
    let needed = SESSION.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|s| s.current.as_ref())
            .is_some_and(|r| !r.reads.contains_key(&key))
    });
    if needed {
        read(key, class_hash(info, &names(), wild));
    }
}

pub(crate) fn read_value(key: String, value: Option<&Ty>) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    read(key, fingerprint::value_hash(value));
}

/// Cache inputs retain provenance and keyed recursive identities, unlike
/// the semantic result digest used by the cold shadow.
pub(crate) fn input_type_hash(value: &Ty) -> u64 {
    fingerprint::value_hash(Some(value))
}

pub(crate) fn read_decl(id: &rubydex::model::ids::DeclarationId, value: Option<&Ty>) {
    if ACTIVE.load(Ordering::Relaxed) {
        read_value(format!("decl:{}", id.get()), value);
    }
}

pub(crate) fn fold_read(key: &super::fold::SlotKey, value: Option<&Ty>) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    if let Some(key) = wire::slot_key(key) {
        read_value(format!("fold:{}", key), value);
    }
}

pub(crate) fn fold_write(key: &super::fold::SlotKey, before: Option<&Ty>, after: &Ty) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let _record = phase(Phase::Record);
    fold_read(key, before);
    let Some(key) = wire::slot_key(key) else {
        return;
    };
    let Some(value) = encode(serde_json::to_value(after).unwrap()) else {
        return;
    };
    SESSION.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut().and_then(|s| s.current.as_mut()) {
            r.fold_writes.push((key, value));
        }
    });
}

pub(crate) struct Pending {
    dir: PathBuf,
    cache: Cache,
    pub(crate) stats: Stats,
    pub(crate) profile: Option<Value>,
}

/// Stop recording before starting the cold shadow. The previous cache is
/// left intact until the complete-state comparison has passed.
pub(crate) fn stop() -> Option<Pending> {
    ACTIVE.store(false, Ordering::Relaxed);
    SESSION.with(|s| s.borrow_mut().take()).map(|mut s| {
        s.stats.unused = s.old.len() - s.cursor;
        s.stats.recorded = s.cache.records.len();
        let mut pending = Pending {
            dir: s.dir,
            cache: s.cache,
            stats: s.stats,
            profile: None,
        };
        let cleanup = phase(Phase::Cleanup);
        drop(s.old);
        drop(s.units);
        drop(s.sites);
        drop(s.current);
        drop(s.classes_by_idx);
        drop(cleanup);
        pending.profile = profile::stop();
        pending
    })
}

impl Pending {
    pub(crate) fn save(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("create warm cache: {e}"))?;
        let path = self
            .dir
            .join(format!("evaluations.{}.tmp", std::process::id()));
        let file = std::fs::File::create(&path).map_err(|e| format!("write warm cache: {e}"))?;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer(&mut writer, &self.cache)
            .map_err(|e| format!("serialize warm cache: {e}"))?;
        use std::io::Write;
        writer
            .flush()
            .map_err(|e| format!("flush warm cache: {e}"))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|e| format!("sync warm cache: {e}"))?;
        std::fs::rename(&path, self.dir.join("evaluations.json"))
            .map_err(|e| format!("publish warm cache: {e}"))
    }
}

pub(super) fn register(app: &App, units: &[Unit]) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    SESSION.with(|s| {
        let mut s = s.borrow_mut();
        let s = s.as_mut().unwrap();
        let mut occurrences = HashMap::<String, usize>::new();
        for (id, u) in units.iter().enumerate() {
            let file = super::sccq::unit_body(app, u.family, u.ci, u.mi)
                .span
                .file
                .0;
            let path = file
                .checked_sub(1)
                .and_then(|f| app.sources.get(f as usize))
                .map(|source| source.path.as_str())
                .unwrap_or("synthetic");
            let prefix = format!(
                "{:?}:{}:{}:{}",
                u.family,
                u.class.0.as_str(),
                if u.class_side { "class" } else { "instance" },
                path
            );
            // Reopened classes and repeated definitions can have the same
            // semantic name. Keep their writers and source sites distinct.
            let occurrence = occurrences
                .entry(format!("{prefix}:{}", u.name.as_str()))
                .or_default();
            let key = format!("{prefix}:{}:{}", *occurrence, u.name.as_str());
            *occurrence += 1;
            let body = fingerprint::unit(app, u);
            s.sites.add_unit(app, u, &key);
            s.units.insert(id as u32, (key, body, u.names.clone()));
        }
        let valid: HashMap<_, _> = s.units.values().map(|(k, b, _)| (k.clone(), *b)).collect();
        s.old.retain(|r| valid.get(&r.unit) == Some(&r.body));
        s.stats.invalidated = s.stats.loaded - s.old.len();
    });
}

pub(super) fn begin_unit(unit: Option<u32>) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    SESSION.with(|s| {
        if let Some(s) = s.borrow_mut().as_mut() {
            s.unit = unit;
            s.root = 0;
        }
    });
}

pub(super) fn end_unit() {
    begin_unit(None);
}

pub(super) struct UnitScope;
impl Drop for UnitScope {
    fn drop(&mut self) {
        end_unit();
    }
}

pub(crate) fn extra_needed() -> bool {
    ACTIVE.load(Ordering::Relaxed)
        && SESSION.with(|s| {
            s.borrow()
                .as_ref()
                .is_some_and(|s| s.depth == 0 && s.unit.is_some())
        })
}

/// One outer body/default evaluation. Recursive expression visits are part
/// of the same record, rather than separate cache entries.
pub(crate) struct Evaluation {
    outer: bool,
    typing_root: bool,
    pub(crate) replay: Option<Expr>,
    before_reads: Option<super::sccq::Reads>,
}

impl Drop for Evaluation {
    fn drop(&mut self) {
        if let Some(before) = self.before_reads.take() {
            super::sccq::warm_rec_restore(before);
        }
        SESSION.with(|s| {
            if let Some(s) = s.borrow_mut().as_mut() {
                s.depth -= 1;
            }
        });
    }
}

impl Evaluation {
    pub(crate) fn begin(
        expr: &Expr,
        ctx: &Ctx,
        classes: &HashMap<ClassId, ClassInfo>,
        extra: Value,
    ) -> Option<Self> {
        if !ACTIVE.load(Ordering::Relaxed) {
            return None;
        }
        let (typing_root, outer) = SESSION.with(|s| {
            let mut s = s.borrow_mut();
            let s = s.as_mut().unwrap();
            s.depth += 1;
            (s.depth == 1, s.depth == 1 && s.unit.is_some())
        });
        let mut eval = Self {
            outer,
            typing_root,
            replay: None,
            before_reads: if outer {
                super::sccq::warm_rec_take()
            } else {
                None
            },
        };
        if !outer {
            return Some(eval);
        }
        let guard = phase(Phase::Guard);
        SESSION.with(|s| {
            let mut s = s.borrow_mut();
            let s = s.as_mut().unwrap();
            if s.classes_by_idx.is_empty() {
                s.classes_by_idx = classes
                    .iter()
                    .filter(|(_, c)| c.sccq_idx != 0)
                    .map(|(id, c)| (c.sccq_idx, id.0.as_str().to_owned()))
                    .collect();
            }
        });
        let input = inputs(expr, ctx, classes, &extra);
        let candidate = SESSION.with(|s| {
            let mut s = s.borrow_mut();
            let s = s.as_mut().unwrap();
            let (key, body, _) = &s.units[&s.unit.unwrap()];
            let root = s.root;
            s.root += 1;
            let candidate = s
                .old
                .get(s.cursor)
                .filter(|r| &r.unit == key && r.root == root)
                .cloned();
            if candidate.is_some() {
                s.cursor += 1;
            }
            s.current = Some(Record {
                unit: key.clone(),
                body: *body,
                root,
                input,
                reads: BTreeMap::new(),
                write: Value::Null,
                fold_writes: Vec::new(),
                scheduler: None,
            });
            candidate
        });
        let covered = candidate.as_ref().is_some_and(|r| {
            r.input == input && fingerprint::covered(&r.reads, ctx, classes, &extra)
        });
        // ConstScope guards can call S3's recorder. They are validation,
        // not reads made by the transfer, so discard this scratch set.
        let _ = super::sccq::warm_rec_take();
        if let Some(r) = candidate {
            if covered && r.scheduler.as_ref().is_some_and(|r| r.ready(classes)) {
                if !wire::ready(&r.write, &r.fold_writes) {
                    SESSION.with(|s| s.borrow_mut().as_mut().unwrap().stats.uncovered += 1);
                    SESSION.with(|s| s.borrow_mut().as_mut().unwrap().stats.typed += 1);
                    return Some(eval);
                }
                drop(guard);
                let _replay = phase(Phase::Replay);
                if let Some(body) = wire::restore(&r.write, expr) {
                    if wire::apply_fold_writes(&r.fold_writes) {
                        r.scheduler.as_ref().unwrap().replay(classes);
                        SESSION.with(|s| {
                            let mut s = s.borrow_mut();
                            let s = s.as_mut().unwrap();
                            s.stats.replayed += 1;
                            s.current = Some(r);
                        });
                        eval.replay = Some(body);
                        return Some(eval);
                    }
                }
            }
            SESSION.with(|s| s.borrow_mut().as_mut().unwrap().stats.uncovered += 1);
        }
        SESSION.with(|s| s.borrow_mut().as_mut().unwrap().stats.typed += 1);
        Some(eval)
    }

    pub(crate) fn fresh(&self) -> bool {
        self.typing_root && self.replay.is_none()
    }

    pub(crate) fn finish(&self, expr: &Expr) {
        if !self.outer {
            return;
        }
        let _record = phase(Phase::Record);
        let output = if self.replay.is_none() {
            encode(serde_json::to_value(expr).expect("expression JSON"))
        } else {
            None
        };
        let scheduler = if self.replay.is_none() {
            super::sccq::warm_rec_snapshot().and_then(SchedulerReads::capture)
        } else {
            None
        };
        SESSION.with(|s| {
            let mut s = s.borrow_mut();
            let s = s.as_mut().unwrap();
            if let Some(mut r) = s.current.take() {
                if self.replay.is_none() {
                    r.write = output.unwrap_or(Value::Null);
                    r.scheduler = scheduler;
                }
                s.cache.records.push(r);
            }
        });
    }
}
