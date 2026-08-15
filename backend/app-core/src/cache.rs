//! 本地缓存与 LRU 清理策略。
//!
//! 对应需求 3.1「本地缓存与 Pin 管理，支持 LRU 或定期清理策略」：
//! - [`LruCache`]：基于条目数的通用 LRU（最近最少使用）缓存；
//! - [`ContentCache`]：面向 IPFS 内容的字节数受限缓存（CID → 字节），
//!   超出容量时按 LRU 顺序逐出，直至回到阈值以下。
//!
//! 缓存为纯内存实现，无外部依赖，便于单元测试与跨平台（含移动端）使用。

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

/// 通用 LRU 缓存（按条目数限制容量）。
pub struct LruCache<K, V> {
    capacity: usize,
    map: HashMap<K, V>,
    order: VecDeque<K>,
}

impl<K: Clone + Eq + Hash, V> LruCache<K, V> {
    /// 创建容量为 `capacity` 的缓存。
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// 当前条目数。
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// 容量上限。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 是否包含键。
    pub fn contains_key(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    /// 读取值，并将键标记为最近使用。
    pub fn get(&mut self, key: &K) -> Option<&V> {
        if self.map.contains_key(key) {
            self.touch(key);
            self.map.get(key)
        } else {
            None
        }
    }

    /// 写入键值；若超过容量则逐出最久未使用的条目。
    ///
    /// 返回被逐出的 (键, 值)（若有）。
    pub fn insert(&mut self, key: K, value: V) -> Option<(K, V)> {
        if !self.map.contains_key(&key) {
            self.order.push_back(key.clone());
        } else {
            self.touch(&key);
        }
        self.map.insert(key, value);
        if self.map.len() > self.capacity {
            self.evict_one()
        } else {
            None
        }
    }

    /// 移除键，返回其值（若有）。
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let v = self.map.remove(key)?;
        self.order.retain(|k| k != key);
        Some(v)
    }

    /// 清空缓存。
    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    /// 逐出最久未使用的一个条目，返回被逐出的 (键, 值)。
    pub fn evict_one(&mut self) -> Option<(K, V)> {
        let oldest = self.order.pop_front()?;
        let v = self.map.remove(&oldest)?;
        Some((oldest, v))
    }

    /// 将键移动到「最近使用」位置。
    fn touch(&mut self, key: &K) {
        self.order.retain(|k| k != key);
        self.order.push_back(key.clone());
    }
}

/// 面向 IPFS 内容的字节数受限缓存（CID → 字节）。
///
/// 以条目数（委托给内部 [`LruCache`]）+ 总字节数双重上限做 LRU 逐出。
pub struct ContentCache {
    inner: LruCache<String, Vec<u8>>,
    max_bytes: usize,
    total_bytes: usize,
}

impl ContentCache {
    /// 创建缓存。`max_entries` 为条目上限，`max_bytes` 为总字节上限。
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            inner: LruCache::new(max_entries),
            max_bytes,
            total_bytes: 0,
        }
    }

    /// 当前条目数。
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// 当前总字节数。
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// 是否包含该 CID。
    pub fn contains(&self, cid: &str) -> bool {
        self.inner.contains_key(&cid.to_string())
    }

    /// 读取内容（标记为最近使用）。
    pub fn get(&mut self, cid: &str) -> Option<&[u8]> {
        self.inner.get(&cid.to_string()).map(|v| v.as_slice())
    }

    /// 写入内容，超出字节上限则逐出最久未使用条目直至回落到限制内。
    pub fn insert(&mut self, cid: impl Into<String>, bytes: Vec<u8>) {
        let cid = cid.into();
        // 替换已有条目：先扣除旧字节。
        if let Some(old) = self.inner.remove(&cid) {
            self.total_bytes = self.total_bytes.saturating_sub(old.len());
        }
        self.total_bytes += bytes.len();
        // 条目数超限由内部 LruCache 处理；这里只需关注字节超限。
        self.inner.insert(cid, bytes);
        while self.total_bytes > self.max_bytes {
            let Some((_, v)) = self.inner.evict_one() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(v.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut c = LruCache::new(2);
        c.insert("a", 1);
        c.insert("b", 2);
        // 访问 a 使其成为最近使用。
        assert_eq!(c.get(&"a"), Some(&1));
        // 插入 c，应逐出 b（最久未使用）。
        let evicted = c.insert("c", 3);
        assert_eq!(evicted, Some(("b", 2)));
        assert!(c.contains_key(&"a"));
        assert!(c.contains_key(&"c"));
        assert!(!c.contains_key(&"b"));
    }

    #[test]
    fn lru_get_updates_recency() {
        let mut c = LruCache::new(2);
        c.insert(1, "one");
        c.insert(2, "two");
        assert_eq!(c.get(&1), Some(&"one"));
        c.insert(3, "three");
        assert!(c.contains_key(&1));
        assert!(!c.contains_key(&2));
        assert!(c.contains_key(&3));
    }

    #[test]
    fn lru_remove_and_clear() {
        let mut c = LruCache::new(3);
        c.insert("x", 1);
        c.insert("y", 2);
        assert_eq!(c.remove(&"x"), Some(1));
        assert!(!c.contains_key(&"x"));
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn content_cache_respects_byte_limit() {
        let mut c = ContentCache::new(10, 6);
        c.insert("a", vec![0u8; 4]);
        c.insert("b", vec![0u8; 4]); // 总计 8 > 6，应逐出 a。
        assert_eq!(c.total_bytes(), 4);
        assert!(c.contains("b"));
        assert!(!c.contains("a"));
    }

    #[test]
    fn content_cache_replaces_entry() {
        let mut c = ContentCache::new(10, 100);
        c.insert("cid", vec![0u8; 3]);
        c.insert("cid", vec![0u8; 5]); // 替换旧条目。
        assert_eq!(c.total_bytes(), 5);
        assert_eq!(c.get("cid"), Some(&[0u8; 5][..]));
    }
}
