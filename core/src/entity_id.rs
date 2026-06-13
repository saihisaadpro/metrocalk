//! Peer-namespaced entity identifiers (F3 from loro spike).
//!
//! Concurrent entity creation on different replicas must never collide. Loro's own `TreeID` is
//! `(peer, counter)` and never collides; our application-level `EntityId` mirrors that shape so
//! the mapping is 1:1.

use std::fmt;

/// A globally unique, peer-namespaced entity identifier. Two replicas creating entities
/// concurrently will never produce the same `EntityId` because each embeds its own `peer` id.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct EntityId {
    pub peer: u64,
    pub counter: u64,
}

impl EntityId {
    /// The string key used in the Loro document (components map, binding keys, tree-node meta).
    pub fn to_loro_key(&self) -> String {
        format!("{:x}_{:x}", self.peer, self.counter)
    }

    /// Parse from the Loro key format produced by [`to_loro_key`](Self::to_loro_key).
    pub fn from_loro_key(s: &str) -> Option<Self> {
        let (peer_s, counter_s) = s.split_once('_')?;
        Some(Self {
            peer: u64::from_str_radix(peer_s, 16).ok()?,
            counter: u64::from_str_radix(counter_s, 16).ok()?,
        })
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}_{:x}", self.peer, self.counter)
    }
}

/// Allocates peer-namespaced [`EntityId`]s. Each peer has its own generator with a unique `peer`
/// value; the `counter` increments monotonically.
pub struct IdGenerator {
    peer: u64,
    counter: u64,
}

impl IdGenerator {
    pub fn new(peer: u64) -> Self {
        Self { peer, counter: 0 }
    }

    pub fn next_id(&mut self) -> EntityId {
        let id = EntityId {
            peer: self.peer,
            counter: self.counter,
        };
        self.counter += 1;
        id
    }

    pub fn peer(&self) -> u64 {
        self.peer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let id = EntityId {
            peer: 0xABCD,
            counter: 42,
        };
        let key = id.to_loro_key();
        assert_eq!(key, "abcd_2a");
        assert_eq!(EntityId::from_loro_key(&key), Some(id));
    }

    #[test]
    fn no_collision_across_peers() {
        let mut g1 = IdGenerator::new(1);
        let mut g2 = IdGenerator::new(2);
        let ids: Vec<EntityId> = (0..100)
            .map(|_| g1.next_id())
            .chain((0..100).map(|_| g2.next_id()))
            .collect();
        let set: std::collections::HashSet<EntityId> = ids.iter().copied().collect();
        assert_eq!(set.len(), 200);
    }
}
