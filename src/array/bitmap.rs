//! NULL 位图:用 `Vec<u64>` 位压缩存储每个位置是否有效(非 NULL)。
//!
//! 这是贯穿全引擎的复杂点——所有算子和表达式求值都必须正确传播 NULL 语义。

/// 有效位位图。第 `i` 位为 1 表示位置 `i` 非 NULL。
#[derive(Debug, Clone, Default)]
pub struct Validity {
    bits: Vec<u64>,
    len: usize,
}

impl Validity {
    /// 空位图。
    pub fn new() -> Self {
        Validity::default()
    }

    /// 预留 `cap` 个位置的容量。
    pub fn with_capacity(cap: usize) -> Self {
        Validity {
            bits: Vec::with_capacity(cap.div_ceil(64)),
            len: 0,
        }
    }

    /// 构造长度为 `len` 且全部有效的位图。
    pub fn all_valid(len: usize) -> Self {
        let words = len.div_ceil(64);
        let mut bits = vec![u64::MAX; words];
        // 清掉最后一个 word 中超出 len 的高位,保证 count_nulls 正确。
        if !len.is_multiple_of(64) {
            let last = len / 64;
            let valid_bits = len % 64;
            bits[last] = (1u64 << valid_bits) - 1;
        }
        Validity { bits, len }
    }

    /// 追加一个位。
    pub fn push(&mut self, valid: bool) {
        let word = self.len / 64;
        let bit = self.len % 64;
        if word >= self.bits.len() {
            self.bits.push(0);
        }
        if valid {
            self.bits[word] |= 1u64 << bit;
        }
        self.len += 1;
    }

    /// 位置 `i` 是否有效(非 NULL)。
    pub fn is_valid(&self, i: usize) -> bool {
        let word = i / 64;
        let bit = i % 64;
        (self.bits[word] >> bit) & 1 == 1
    }

    /// 设置位置 `i` 的有效性。
    pub fn set(&mut self, i: usize, valid: bool) {
        let word = i / 64;
        let bit = i % 64;
        if valid {
            self.bits[word] |= 1u64 << bit;
        } else {
            self.bits[word] &= !(1u64 << bit);
        }
    }

    /// 位图长度。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// NULL(无效位)个数。
    pub fn count_nulls(&self) -> usize {
        let valid: usize = self.bits.iter().map(|w| w.count_ones() as usize).sum();
        self.len - valid
    }

    /// 把另一个位图的全部位追加到自身末尾(用于 concat)。
    pub fn extend_from(&mut self, other: &Validity) {
        for i in 0..other.len {
            self.push(other.is_valid(i));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_query() {
        let mut v = Validity::new();
        for &b in &[true, false, true, true, false] {
            v.push(b);
        }
        assert_eq!(v.len(), 5);
        assert!(v.is_valid(0));
        assert!(!v.is_valid(1));
        assert!(v.is_valid(3));
        assert_eq!(v.count_nulls(), 2);
    }

    #[test]
    fn all_valid_no_phantom_bits() {
        let v = Validity::all_valid(3);
        assert_eq!(v.len(), 3);
        assert_eq!(v.count_nulls(), 0);
        let v2 = Validity::all_valid(70);
        assert_eq!(v2.count_nulls(), 0);
    }

    #[test]
    fn set_toggles() {
        let mut v = Validity::all_valid(64);
        v.set(10, false);
        assert!(!v.is_valid(10));
        assert_eq!(v.count_nulls(), 1);
        v.set(10, true);
        assert_eq!(v.count_nulls(), 0);
    }
}
