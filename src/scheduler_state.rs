//! Read-only Scheduler dashboard projections.
//!
//! This module bounds the persisted scheduler-cycle history shown on the
//! Execution dashboard. It does not affect scheduler cadence, work execution,
//! retention, or any broker-facing behavior.

pub(crate) const SCHEDULER_CYCLES_PAGE_SIZE: i64 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SchedulerCyclePage {
    pub(crate) page: i64,
    pub(crate) offset: i64,
}

pub(crate) fn scheduler_cycle_page(requested_page: i64, total_cycles: i64) -> SchedulerCyclePage {
    let total_pages = ((total_cycles.max(0) + SCHEDULER_CYCLES_PAGE_SIZE - 1)
        / SCHEDULER_CYCLES_PAGE_SIZE)
        .max(1);
    let page = requested_page.max(1).min(total_pages);
    SchedulerCyclePage {
        page,
        offset: (page - 1) * SCHEDULER_CYCLES_PAGE_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_page_and_calculates_offset() {
        assert_eq!(
            scheduler_cycle_page(2, 25),
            SchedulerCyclePage {
                page: 2,
                offset: SCHEDULER_CYCLES_PAGE_SIZE,
            }
        );
        assert_eq!(
            scheduler_cycle_page(9, 13),
            SchedulerCyclePage {
                page: 2,
                offset: SCHEDULER_CYCLES_PAGE_SIZE,
            }
        );
        assert_eq!(
            scheduler_cycle_page(0, 0),
            SchedulerCyclePage { page: 1, offset: 0 }
        );
    }
}
