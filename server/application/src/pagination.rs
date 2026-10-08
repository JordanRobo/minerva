//! Pagination shared by the list endpoints (roadmap 3.10, D16): limit/offset
//! with a total count, answered as a [`Page`] envelope. The default page size
//! and the maximum live here so every endpoint agrees on them.

/// An error from validating a [`PageRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageRequestError {
    /// `limit` was outside `1..=PageRequest::MAX_PAGE_LIMIT`.
    LimitOutOfRange(i32),
    /// `offset` was negative.
    NegativeOffset(i64),
}

impl std::fmt::Display for PageRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PageRequestError::LimitOutOfRange(_) => write!(
                f,
                "limit must be a whole number between 1 and {}",
                PageRequest::MAX_PAGE_LIMIT
            ),
            PageRequestError::NegativeOffset(_) => write!(f, "offset must not be negative"),
        }
    }
}

impl std::error::Error for PageRequestError {}

/// One page of a list: how many rows to fetch and where to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    /// Rows per page; `1..=MAX_PAGE_LIMIT`.
    pub limit: u32,
    /// Rows to skip before the first result; `0` for the first page.
    pub offset: u64,
}

impl PageRequest {
    /// The page size applied when the client sends no `limit`.
    pub const DEFAULT_PAGE_LIMIT: u32 = 50;
    /// The largest page size a client may request.
    pub const MAX_PAGE_LIMIT: u32 = 200;

    /// Build a page request from raw (already numeric) values; a `None` field
    /// takes the default (`DEFAULT_PAGE_LIMIT`, offset `0`). The inputs are
    /// signed so an out-of-range value is reported, not truncated.
    pub fn new(limit: Option<i32>, offset: Option<i64>) -> Result<Self, PageRequestError> {
        let limit = match limit {
            None => Self::DEFAULT_PAGE_LIMIT,
            Some(limit) if (1..=Self::MAX_PAGE_LIMIT as i32).contains(&limit) => limit as u32,
            Some(limit) => return Err(PageRequestError::LimitOutOfRange(limit)),
        };
        let offset = match offset {
            None => 0,
            Some(offset) if offset >= 0 => offset as u64,
            Some(offset) => return Err(PageRequestError::NegativeOffset(offset)),
        };
        Ok(Self { limit, offset })
    }
}

/// One page of results plus the total number of matching rows (roadmap 3.10,
/// D16). `total` honours the list's filters but ignores `limit` and `offset`,
/// so every page of the same query reports the same total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// The rows on this page (fewer than `limit` at the end of the list).
    pub items: Vec<T>,
    /// Total matching rows, independent of the page position.
    pub total: u64,
    /// The page size this answer used.
    pub limit: u32,
    /// The offset this answer started at.
    pub offset: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_apply_when_nothing_is_given() {
        let page = PageRequest::new(None, None).expect("defaults are valid");
        assert_eq!(page.limit, PageRequest::DEFAULT_PAGE_LIMIT);
        assert_eq!(page.offset, 0);
    }

    #[test]
    fn the_bounds_are_accepted() {
        assert_eq!(
            PageRequest::new(Some(1), Some(0)).expect("the lower bound is valid"),
            PageRequest {
                limit: 1,
                offset: 0
            }
        );
        let page = PageRequest::new(Some(PageRequest::MAX_PAGE_LIMIT as i32), Some(9_999))
            .expect("the upper bound is valid");
        assert_eq!(page.limit, PageRequest::MAX_PAGE_LIMIT);
        assert_eq!(page.offset, 9_999);
    }

    #[test]
    fn out_of_range_limits_are_rejected() {
        for limit in [0i32, -1, PageRequest::MAX_PAGE_LIMIT as i32 + 1, i32::MAX] {
            assert!(
                matches!(
                    PageRequest::new(Some(limit), None).unwrap_err(),
                    PageRequestError::LimitOutOfRange(_)
                ),
                "limit {limit} must be rejected"
            );
        }
    }

    #[test]
    fn negative_offsets_are_rejected() {
        assert!(matches!(
            PageRequest::new(None, Some(-1)).unwrap_err(),
            PageRequestError::NegativeOffset(_)
        ));
    }
}
