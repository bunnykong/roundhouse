//! `ClaimMap`: a name-keyed store whose reads cannot see the order its writes
//! arrived in (rule O5: writer-keyed, withdrawable output).
//!
//! A slot `K` holds at most one claim per writer `W`. A writer's new claim
//! replaces its own previous claim there and nothing else; a writer that stops
//! writing a slot withdraws its claim (`withdraw`, or `rewrite` with the
//! writer's whole new output); a slot left without claims is absent. Claims
//! are kept in `BTreeMap`s, so the state is a function of the latest claim of
//! each `(slot, writer)` pair: every order of the same writes gives the same
//! state, provided each writer key names one syntactic writer (a key shared by
//! two writers is a single-winner store again).
//!
//! Nothing reads in arrival order. Readers pick one of two policies:
//!
//! - **Join** (`join`, `resolve_join`, and `any`, the join of the boolean
//!   lattice): the join of every current claim. With an associative,
//!   commutative and idempotent join, neither arrival order nor writer order
//!   shows in the result.
//! - **Last** (`last`, `resolve_last`): the claim of the greatest writer. The
//!   writer key's `Ord` must be a Ruby order (include precedence; source
//!   position as file path plus line), never arrival order; the read is then
//!   Ruby's "the later definition wins", whatever order the claims arrived in.

// `withdraw`, `rewrite` and `join` wait for the first store kept across passes.
#![allow(dead_code)]

use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) struct ClaimMap<K, W, V> {
    slots: BTreeMap<K, BTreeMap<W, V>>,
}

impl<K, W, V> Default for ClaimMap<K, W, V> {
    fn default() -> Self {
        ClaimMap { slots: BTreeMap::new() }
    }
}

impl<K: Ord, W: Ord, V> ClaimMap<K, W, V> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Writer `w` claims `v` for slot `k`. Replaces `w`'s previous claim on
    /// `k`, which it returns, and no other writer's.
    pub(crate) fn claim(&mut self, k: K, w: W, v: V) -> Option<V> {
        self.slots.entry(k).or_default().insert(w, v)
    }

    /// Writer `w` no longer writes `k`. A slot left without claims is absent.
    pub(crate) fn withdraw(&mut self, k: &K, w: &W) -> Option<V> {
        let claims = self.slots.get_mut(k)?;
        let old = claims.remove(w);
        if claims.is_empty() {
            self.slots.remove(k);
        }
        old
    }

    /// Re-evaluation of writer `w`: its claims become exactly `output`. Every
    /// slot `w` stops writing loses `w`'s claim; other writers keep theirs.
    pub(crate) fn rewrite(&mut self, w: &W, output: impl IntoIterator<Item = (K, V)>)
    where
        W: Clone,
    {
        self.slots.retain(|_, claims| {
            claims.remove(w);
            !claims.is_empty()
        });
        for (k, v) in output {
            self.claim(k, w.clone(), v);
        }
    }

    /// Whether any writer claims `k`.
    pub(crate) fn contains(&self, k: &K) -> bool {
        self.slots.contains_key(k)
    }

    /// The current claims on `k`, in writer order.
    pub(crate) fn claims<'a>(&'a self, k: &K) -> impl Iterator<Item = (&'a W, &'a V)> + 'a {
        self.slots.get(k).into_iter().flatten()
    }

    /// Policy Last: the claim of `k`'s greatest writer.
    pub(crate) fn last(&self, k: &K) -> Option<&V> {
        self.slots.get(k)?.values().next_back()
    }

    /// Policy Last for every slot, in slot order.
    pub(crate) fn resolve_last(&self) -> impl Iterator<Item = (&K, &V)> {
        self.slots.iter().filter_map(|(k, claims)| Some((k, claims.values().next_back()?)))
    }

    /// Policy Join: the join of every claim on `k`. `join` must be
    /// associative, commutative and idempotent.
    pub(crate) fn join(&self, k: &K, join: impl FnMut(V, &V) -> V) -> Option<V>
    where
        V: Clone,
    {
        let mut claims = self.slots.get(k)?.values();
        let first = claims.next()?.clone();
        Some(claims.fold(first, join))
    }

    /// Policy Join for every slot, in slot order.
    pub(crate) fn resolve_join<'a>(
        &'a self,
        mut join: impl FnMut(V, &V) -> V + 'a,
    ) -> impl Iterator<Item = (&'a K, V)> + 'a
    where
        V: Clone,
    {
        self.slots.iter().filter_map(move |(k, claims)| {
            let mut values = claims.values();
            let first = values.next()?.clone();
            Some((k, values.fold(first, &mut join)))
        })
    }

    /// Policy Join for every slot, consuming the map so no claim is cloned.
    pub(crate) fn into_resolve_join(self, mut join: impl FnMut(V, &V) -> V) -> impl Iterator<Item = (K, V)> {
        self.slots.into_iter().filter_map(move |(k, claims)| {
            let mut values = claims.into_values();
            let first = values.next()?;
            Some((k, values.fold(first, |acc, v| join(acc, &v))))
        })
    }

    /// Policy Join in the boolean lattice: whether `pred` holds of some claim
    /// on `k` (false when no writer claims `k`).
    pub(crate) fn any(&self, k: &K, pred: impl FnMut(&V) -> bool) -> bool {
        self.slots.get(k).is_some_and(|claims| claims.values().any(pred))
    }
}

impl<K: Ord, W: Ord, V> FromIterator<(K, W, V)> for ClaimMap<K, W, V> {
    fn from_iter<I: IntoIterator<Item = (K, W, V)>>(claims: I) -> Self {
        let mut map = ClaimMap::new();
        for (k, w, v) in claims {
            map.claim(k, w, v);
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::ClaimMap;
    use std::collections::{BTreeSet, HashMap};

    /// Seven claims on four slots from five writers; a writer key is (file, line).
    type Claim = (&'static str, (&'static str, u32), &'static str);
    const CLAIMS: [Claim; 7] = [
        ("@x", ("app/a.rb", 3), "Integer"),
        ("@x", ("app/a.rb", 9), "String"),
        ("@x", ("app/b.rb", 1), "Symbol"),
        ("@y", ("app/b.rb", 1), "Integer"),
        ("@y", ("app/a.rb", 3), "Nil"),
        ("@z", ("app/c.rb", 2), "Hash"),
        ("load", ("app/c.rb", 2), "true"),
    ];

    /// Every permutation of `0..n` (Heap's algorithm).
    fn permutations(n: usize) -> Vec<Vec<usize>> {
        fn go(k: usize, a: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
            if k <= 1 {
                out.push(a.clone());
                return;
            }
            for i in 0..k {
                go(k - 1, a, out);
                a.swap(if k % 2 == 0 { i } else { 0 }, k - 1);
            }
        }
        let mut out = Vec::new();
        go(n, &mut (0..n).collect(), &mut out);
        out
    }

    type Set = BTreeSet<&'static str>;
    type Reads = (Vec<(&'static str, &'static str)>, Vec<(&'static str, Set)>, Option<Set>, bool, bool);

    /// Every read the map offers, after inserting `CLAIMS` in `order`.
    fn reads(order: &[usize]) -> Reads {
        let map: ClaimMap<&str, (&str, u32), Set> =
            order.iter().map(|&i| (CLAIMS[i].0, CLAIMS[i].1, Set::from([CLAIMS[i].2]))).collect();
        let union = |a: Set, b: &Set| a.union(b).copied().collect::<Set>();
        (
            map.resolve_last().map(|(k, v)| (*k, *v.first().unwrap())).collect(),
            map.resolve_join(union).map(|(k, v)| (*k, v)).collect(),
            map.join(&"@y", union),
            map.any(&"@x", |v| v.contains("Symbol")),
            map.any(&"@w", |_| true),
        )
    }

    #[test]
    fn reads_do_not_depend_on_insertion_order() {
        let orders = permutations(CLAIMS.len());
        assert_eq!(orders.len(), 5040);
        assert_eq!(orders.iter().collect::<BTreeSet<_>>().len(), 5040);
        let expected = reads(&orders[0]);
        for order in &orders {
            assert_eq!(reads(order), expected, "insertion order {order:?}");
        }
        // Last: the greatest (file, line), not the latest arrival.
        assert_eq!(expected.0, [("@x", "Symbol"), ("@y", "Integer"), ("@z", "Hash"), ("load", "true")]);
        // Join: every claim.
        assert_eq!(expected.1[0], ("@x", Set::from(["Integer", "String", "Symbol"])));
        assert_eq!(expected.2, Some(Set::from(["Integer", "Nil"])));
        assert!(expected.3 && !expected.4);
    }

    #[test]
    fn a_first_writer_wins_store_does_depend_on_it() {
        // The control: the single-winner form these stores replace, over the same claims.
        let mut first = BTreeSet::new();
        let mut last = BTreeSet::new();
        for order in permutations(CLAIMS.len()) {
            let mut or_insert = HashMap::new();
            let mut insert = HashMap::new();
            for i in order {
                or_insert.entry(CLAIMS[i].0).or_insert(CLAIMS[i].2);
                insert.insert(CLAIMS[i].0, CLAIMS[i].2);
            }
            first.insert(or_insert["@x"]);
            last.insert(insert["@x"]);
        }
        assert_eq!(first, Set::from(["Integer", "String", "Symbol"]));
        assert_eq!(last, first);
    }

    #[test]
    fn a_new_claim_replaces_only_its_own_writer() {
        let mut map = ClaimMap::new();
        assert_eq!(map.claim("@x", 1, "Integer"), None);
        assert_eq!(map.claim("@x", 2, "String"), None);
        assert_eq!(map.claim("@x", 1, "Symbol"), Some("Integer"));
        assert_eq!(map.claims(&"@x").collect::<Vec<_>>(), [(&1, &"Symbol"), (&2, &"String")]);
        assert_eq!(map.withdraw(&"@x", &2), Some("String"));
        assert_eq!(map.last(&"@x"), Some(&"Symbol"));
        assert_eq!(map.withdraw(&"@x", &1), Some("Symbol"));
        assert!(!map.contains(&"@x"));
        assert_eq!(map.last(&"@x"), None);
        assert_eq!(map.withdraw(&"@x", &1), None);
    }

    #[test]
    fn rewrite_withdraws_what_a_writer_stops_writing() {
        let mut map = ClaimMap::new();
        map.rewrite(&"a", [("@x", 1), ("@y", 1)]);
        map.rewrite(&"b", [("@y", 2)]);
        map.rewrite(&"a", [("@x", 3)]);
        assert_eq!(map.claims(&"@x").collect::<Vec<_>>(), [(&"a", &3)]);
        assert_eq!(map.claims(&"@y").collect::<Vec<_>>(), [(&"b", &2)]);
        map.rewrite(&"b", []);
        assert!(!map.contains(&"@y"));
        assert_eq!(map.resolve_last().collect::<Vec<_>>(), [(&"@x", &3)]);
    }

    #[test]
    fn writers_reevaluating_in_any_interleaving_reach_one_state() {
        // Each writer re-evaluates twice, its output changing; only its latest output counts.
        let a: [&[(&str, i32)]; 2] = [&[("@x", 1), ("@y", 1)], &[("@x", 2)]];
        let b: [&[(&str, i32)]; 2] = [&[("@y", 3)], &[("@y", 4), ("@z", 4)]];
        let mut states = BTreeSet::new();
        for picks in [[0, 0, 1, 1], [0, 1, 0, 1], [0, 1, 1, 0], [1, 0, 0, 1], [1, 0, 1, 0], [1, 1, 0, 0]] {
            let mut map = ClaimMap::new();
            let (mut i, mut j) = (0, 0);
            for p in picks {
                if p == 0 {
                    map.rewrite(&'a', a[i].iter().copied());
                    i += 1;
                } else {
                    map.rewrite(&'b', b[j].iter().copied());
                    j += 1;
                }
            }
            let state: Vec<_> = ["@x", "@y", "@z"]
                .iter()
                .map(|k| map.claims(k).map(|(w, v)| (*w, *v)).collect::<Vec<_>>())
                .collect();
            states.insert(state);
        }
        assert_eq!(states.len(), 1);
        assert_eq!(states.first().unwrap(), &vec![vec![('a', 2)], vec![('b', 4)], vec![('b', 4)]]);
    }
}
