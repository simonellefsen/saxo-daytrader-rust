//! Read-only Markov dashboard projections.
//!
//! Pagination here is deliberately independent of the Markov regime model,
//! stored signals, scheduler, and trading gates. It only bounds what the
//! dashboard asks the persisted-signal reader to display.

pub(crate) const MARKOV_SIGNALS_PAGE_SIZE: i64 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MarkovSignalPage {
    pub(crate) page: i64,
    pub(crate) offset: i64,
}

pub(crate) fn markov_signal_page(requested_page: i64, total_signals: i64) -> MarkovSignalPage {
    let total_pages =
        ((total_signals.max(0) + MARKOV_SIGNALS_PAGE_SIZE - 1) / MARKOV_SIGNALS_PAGE_SIZE).max(1);
    let page = requested_page.max(1).min(total_pages);
    MarkovSignalPage {
        page,
        offset: (page - 1) * MARKOV_SIGNALS_PAGE_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_requested_page_and_uses_the_bounded_signal_offset() {
        assert_eq!(
            markov_signal_page(2, 81),
            MarkovSignalPage {
                page: 2,
                offset: MARKOV_SIGNALS_PAGE_SIZE,
            }
        );
        assert_eq!(
            markov_signal_page(9, 41),
            MarkovSignalPage {
                page: 2,
                offset: MARKOV_SIGNALS_PAGE_SIZE,
            }
        );
        assert_eq!(
            markov_signal_page(0, 0),
            MarkovSignalPage { page: 1, offset: 0 }
        );
    }
}
