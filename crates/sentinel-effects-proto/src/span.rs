//! Source spans for Sentinel-Mini (B1).
//!
//! A [`Span`] is a half-open byte range `[start, end)` into the source
//! string. `u32` rather than `usize` because Sentinel-Mini source files
//! are not going to exceed 4 GiB and halving the footprint keeps spanned
//! ASTs cheap.
//!
//! [`Spanned<T>`] is the wrapper used to attach a span to any AST or
//! diagnostic payload. The AST refactor in B1.2 will wrap `ExprKind` as
//! `Spanned<ExprKind>` per the decision recorded with the B1 scope
//! proposal.

/// A half-open byte span `[start, end)` into the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    /// Construct a span from `usize` byte offsets. Panics in debug if
    /// either offset overflows `u32`; truncates in release. Sentinel-Mini
    /// programs are tiny, so this is fine in practice.
    #[inline]
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end, "Span::new: start > end ({start} > {end})");
        debug_assert!(end <= u32::MAX as usize, "Span::new: end exceeds u32::MAX");
        Span { start: start as u32, end: end as u32 }
    }

    /// A zero-length span at the given offset. Useful as a sentinel value
    /// in tests and as the span for synthetic AST nodes.
    #[inline]
    pub fn point(offset: usize) -> Self {
        Self::new(offset, offset)
    }

    /// The smallest span enclosing both `self` and `other`.
    #[inline]
    pub fn merge(self, other: Span) -> Self {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    #[inline]
    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    #[inline]
    pub fn start_usize(self) -> usize {
        self.start as usize
    }

    #[inline]
    pub fn end_usize(self) -> usize {
        self.end as usize
    }
}

/// A value paired with the source span it came from.
///
/// Used to attach spans to AST nodes and diagnostics without polluting
/// every enum variant with a `span` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    #[inline]
    pub fn new(node: T, span: Span) -> Self {
        Spanned { node, span }
    }

    /// Map the inner node while preserving the span.
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Spanned<U> {
        Spanned { node: f(self.node), span: self.span }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_new_and_accessors() {
        let s = Span::new(3, 10);
        assert_eq!(s.start, 3);
        assert_eq!(s.end, 10);
        assert_eq!(s.len(), 7);
        assert!(!s.is_empty());
        assert_eq!(s.start_usize(), 3);
        assert_eq!(s.end_usize(), 10);
    }

    #[test]
    fn span_point_is_empty() {
        let s = Span::point(5);
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn span_merge_takes_outer_bounds() {
        let a = Span::new(2, 5);
        let b = Span::new(4, 10);
        let m = a.merge(b);
        assert_eq!(m, Span::new(2, 10));
        // commutative
        assert_eq!(b.merge(a), Span::new(2, 10));
    }

    #[test]
    fn spanned_map_preserves_span() {
        let s = Spanned::new(7i64, Span::new(1, 3));
        let s2 = s.map(|n| n + 1);
        assert_eq!(s2.node, 8);
        assert_eq!(s2.span, Span::new(1, 3));
    }
}
