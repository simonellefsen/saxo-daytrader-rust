//! Read-only Execution dashboard projections.
//!
//! These bounds only select which persisted local execution-order rows the
//! dashboard reads. They deliberately do not change broker synchronization,
//! order lifecycle, reconciliation, or Saxo mutation behavior.

pub(crate) const EXECUTION_ORDERS_PAGE_SIZE: i64 = 25;
pub(crate) const OVERVIEW_EXECUTION_ORDERS_LIMIT: i64 = 12;
pub(crate) const SHARED_EXECUTION_ORDERS_LIMIT: i64 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExecutionOrderWindow {
    pub(crate) page: i64,
    pub(crate) page_size: i64,
    pub(crate) offset: i64,
}

pub(crate) fn execution_order_window(
    active_view: &str,
    requested_page: i64,
    total_orders: i64,
) -> ExecutionOrderWindow {
    if active_view != "execution" {
        let page_size = if active_view == "overview" {
            OVERVIEW_EXECUTION_ORDERS_LIMIT
        } else {
            SHARED_EXECUTION_ORDERS_LIMIT
        };
        return ExecutionOrderWindow {
            page: 1,
            page_size,
            offset: 0,
        };
    }

    let total_pages = ((total_orders.max(0) + EXECUTION_ORDERS_PAGE_SIZE - 1)
        / EXECUTION_ORDERS_PAGE_SIZE)
        .max(1);
    let page = requested_page.max(1).min(total_pages);
    ExecutionOrderWindow {
        page,
        page_size: EXECUTION_ORDERS_PAGE_SIZE,
        offset: (page - 1) * EXECUTION_ORDERS_PAGE_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_execution_and_bounds_other_tabs() {
        assert_eq!(
            execution_order_window("execution", 2, 56),
            ExecutionOrderWindow {
                page: 2,
                page_size: EXECUTION_ORDERS_PAGE_SIZE,
                offset: EXECUTION_ORDERS_PAGE_SIZE,
            }
        );
        assert_eq!(
            execution_order_window("execution", 99, 26),
            ExecutionOrderWindow {
                page: 2,
                page_size: EXECUTION_ORDERS_PAGE_SIZE,
                offset: EXECUTION_ORDERS_PAGE_SIZE,
            }
        );
        assert_eq!(
            execution_order_window("overview", 5, 500),
            ExecutionOrderWindow {
                page: 1,
                page_size: OVERVIEW_EXECUTION_ORDERS_LIMIT,
                offset: 0,
            }
        );
        assert_eq!(
            execution_order_window("markov", 5, 500),
            ExecutionOrderWindow {
                page: 1,
                page_size: SHARED_EXECUTION_ORDERS_LIMIT,
                offset: 0,
            }
        );
    }
}
