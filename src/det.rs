//! Hash collections with a fixed hasher, so iteration order (and therefore
//! every result that depends on it) is identical on every run. std's
//! default `RandomState` reseeds per map and per process, which made some
//! repairs produce different meshes from run to run.

use std::collections::hash_map::DefaultHasher;
use std::hash::BuildHasherDefault;

pub type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<DefaultHasher>>;
pub type HashSet<K> = std::collections::HashSet<K, BuildHasherDefault<DefaultHasher>>;
