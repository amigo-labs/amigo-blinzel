//! Byte-budget LRU used for decoded images (RAM) and GPU textures (VRAM).

use std::collections::HashMap;
use std::hash::Hash;

struct Slot<V> {
    value: V,
    bytes: usize,
    tick: u64,
}

pub struct ByteLru<K, V> {
    map: HashMap<K, Slot<V>>,
    bytes: usize,
    tick: u64,
}

impl<K: Eq + Hash + Clone, V> Default for ByteLru<K, V> {
    fn default() -> Self {
        ByteLru {
            map: HashMap::new(),
            bytes: 0,
            tick: 0,
        }
    }
}

impl<K: Eq + Hash + Clone, V> ByteLru<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn contains(&self, k: &K) -> bool {
        self.map.contains_key(k)
    }

    /// Inserts or replaces; returns the replaced value.
    pub fn insert(&mut self, k: K, value: V, bytes: usize) -> Option<V> {
        self.tick += 1;
        let old = self.map.insert(
            k,
            Slot {
                value,
                bytes,
                tick: self.tick,
            },
        );
        self.bytes += bytes;
        old.map(|s| {
            self.bytes -= s.bytes;
            s.value
        })
    }

    /// Looks up and marks as recently used.
    pub fn get(&mut self, k: &K) -> Option<&V> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(k).map(|s| {
            s.tick = tick;
            &s.value
        })
    }

    /// Looks up without touching recency.
    pub fn peek(&self, k: &K) -> Option<&V> {
        self.map.get(k).map(|s| &s.value)
    }

    pub fn remove(&mut self, k: &K) -> Option<V> {
        self.map.remove(k).map(|s| {
            self.bytes -= s.bytes;
            s.value
        })
    }

    /// Evicts least recently used entries (skipping those `keep` protects)
    /// until at most `budget` bytes remain. Returns what was evicted so the
    /// caller can release resources and tell the scheduler.
    pub fn evict_to(&mut self, budget: usize, keep: impl Fn(&K) -> bool) -> Vec<(K, V)> {
        let mut out = Vec::new();
        if self.bytes <= budget {
            return out;
        }
        let mut order: Vec<(u64, K)> = self
            .map
            .iter()
            .filter(|(k, _)| !keep(k))
            .map(|(k, s)| (s.tick, k.clone()))
            .collect();
        order.sort_unstable_by_key(|(t, _)| *t);
        for (_, k) in order {
            if self.bytes <= budget {
                break;
            }
            if let Some(v) = self.remove(&k) {
                out.push((k, v));
            }
        }
        out
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.map.keys()
    }

    pub fn clear(&mut self) -> Vec<(K, V)> {
        self.bytes = 0;
        self.map.drain().map(|(k, s)| (k, s.value)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used_except_kept() {
        let mut c = ByteLru::new();
        c.insert(1, "a", 10);
        c.insert(2, "b", 10);
        c.insert(3, "c", 10);
        assert_eq!(c.bytes(), 30);
        c.get(&1);
        let ev = c.evict_to(20, |k| *k == 2);
        assert_eq!(ev, vec![(3, "c")]);
        assert_eq!(c.bytes(), 20);
        // Everything protected: over budget is allowed.
        assert!(c.evict_to(0, |_| true).is_empty());
        assert_eq!(c.evict_to(0, |_| false).len(), 2);
        assert!(c.is_empty());
    }

    #[test]
    fn replace_and_remove_track_bytes() {
        let mut c = ByteLru::new();
        assert_eq!(c.insert("k", 1, 100), None);
        assert_eq!(c.insert("k", 2, 40), Some(1));
        assert_eq!(c.bytes(), 40);
        assert_eq!(c.peek(&"k"), Some(&2));
        assert_eq!(c.remove(&"k"), Some(2));
        assert_eq!(c.bytes(), 0);
    }
}
