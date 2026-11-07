//! Session_Records: Last_Result and Recent_Use for this TUI process. They live in memory only
//! and are never written to a file (Requirement 16.3). Nothing here reads the clock; callers
//! pass the instants, so tests and snapshot fixtures can fix them.
use crate::runtime::Outcome;
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

/// Recent_Use keeps at most this many Actions.
const RECENT_LIMIT: usize = 10;

/// How the latest run of an Action in this session ended, when, and how long it took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LastResult {
    pub outcome: Outcome,
    pub finished: Instant,
    pub elapsed: Duration,
}

#[derive(Debug, Default)]
pub(super) struct SessionRecords {
    /// Last_Result by Action ID.
    pub last: BTreeMap<&'static str, LastResult>,
    /// Recent_Use: Action IDs by most recent start, newest first, each at most once.
    pub recent: VecDeque<&'static str>,
}

impl SessionRecords {
    /// An Action started: it moves to the front of Recent_Use (Requirement 16.8).
    pub fn started(&mut self, id: &'static str) {
        self.recent.retain(|recent| *recent != id);
        self.recent.push_front(id);
        self.recent.truncate(RECENT_LIMIT);
    }
    /// An Action ended at `at` after running for `elapsed`; this replaces its previous
    /// Last_Result (Requirement 16.9).
    pub fn finished(&mut self, id: &'static str, outcome: Outcome, elapsed: Duration, at: Instant) {
        self.last.insert(
            id,
            LastResult {
                outcome,
                finished: at,
                elapsed,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    /// 15 real Action IDs: enough that sequences both repeat IDs and overflow Recent_Use.
    fn ids() -> Vec<&'static str> {
        let ids = crate::actions::specs()
            .iter()
            .map(|spec| spec.id)
            .take(15)
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 15);
        ids
    }

    /// The model of Recent_Use: IDs by their last occurrence, newest first, then the first 10.
    fn newest_first(started: &[&'static str]) -> Vec<&'static str> {
        let mut order = Vec::new();
        for id in started.iter().rev() {
            if !order.contains(id) {
                order.push(*id);
            }
        }
        order.truncate(RECENT_LIMIT);
        order
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: tui-redesign, Property 14: Recent_Use
        /// **Validates: Requirements 16.8**
        #[test]
        fn recent_use_is_newest_first_without_repeats(
            started in proptest::collection::vec(proptest::sample::select(ids()), 0..40),
        ) {
            let mut records = SessionRecords::default();
            prop_assert!(records.recent.is_empty());
            // Every prefix is itself a sequence of the property, so check after each start.
            for (n, &id) in started.iter().enumerate() {
                records.started(id);
                let recent = records.recent.iter().copied().collect::<Vec<_>>();
                prop_assert!(recent.len() <= RECENT_LIMIT, "{recent:?}");
                prop_assert_eq!(
                    recent.iter().collect::<BTreeSet<_>>().len(),
                    recent.len(),
                    "repeated ID in {:?}",
                    recent
                );
                prop_assert_eq!(recent.first(), Some(&id));
                prop_assert_eq!(&recent, &newest_first(&started[..=n]));
            }
            prop_assert!(records.last.is_empty(), "starting never records a result");
        }
    }

    #[test]
    fn finished_replaces_the_previous_result() {
        let mut records = SessionRecords::default();
        let start = Instant::now();
        let later = start + Duration::from_secs(480);
        records.finished("tcp.tune", Outcome::Failed, Duration::from_secs(3), start);
        records.finished("bbr.enable", Outcome::Done, Duration::from_secs(1), start);
        records.finished(
            "tcp.tune",
            Outcome::Cancelled,
            Duration::from_secs(48),
            later,
        );
        assert_eq!(
            records.last["tcp.tune"],
            LastResult {
                outcome: Outcome::Cancelled,
                finished: later,
                elapsed: Duration::from_secs(48),
            }
        );
        assert_eq!(records.last.len(), 2);
        assert_eq!(records.last["bbr.enable"].outcome, Outcome::Done);
        assert!(
            records.recent.is_empty(),
            "a result does not start an Action"
        );
    }
}
