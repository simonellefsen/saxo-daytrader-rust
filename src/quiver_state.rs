//! Read-only Quiver dashboard projections.
//!
//! Pagination is intentionally separate from Quiver API collection, persisted
//! signals, scheduler execution, and any trading advice derived from them.

pub(crate) const QUIVER_SIGNALS_PAGE_SIZE: i64 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuiverSignalPage {
    pub(crate) page: i64,
    pub(crate) offset: i64,
}

pub(crate) fn quiver_signal_page(requested_page: i64, total_signals: i64) -> QuiverSignalPage {
    let total_pages =
        ((total_signals.max(0) + QUIVER_SIGNALS_PAGE_SIZE - 1) / QUIVER_SIGNALS_PAGE_SIZE).max(1);
    let page = requested_page.max(1).min(total_pages);
    QuiverSignalPage {
        page,
        offset: (page - 1) * QUIVER_SIGNALS_PAGE_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_requested_page_and_uses_the_bounded_signal_offset() {
        assert_eq!(
            quiver_signal_page(2, 81),
            QuiverSignalPage {
                page: 2,
                offset: QUIVER_SIGNALS_PAGE_SIZE,
            }
        );
        assert_eq!(
            quiver_signal_page(9, 41),
            QuiverSignalPage {
                page: 2,
                offset: QUIVER_SIGNALS_PAGE_SIZE,
            }
        );
        assert_eq!(
            quiver_signal_page(0, 0),
            QuiverSignalPage { page: 1, offset: 0 }
        );
    }
}
