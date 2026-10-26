// SPDX-License-Identifier: GPL-3.0-or-later
//! Request ordering for fit callbacks; GTK owns displayed results and errors.

#[derive(Default, Debug)]
pub(crate) struct FitState {
    generation: u64,
    in_flight: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Completion {
    Ignored,
    Stale,
    Current,
}

impl FitState {
    pub(crate) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub(crate) fn begin(&mut self) -> Option<u64> {
        if self.in_flight.is_some() {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.in_flight = Some(self.generation);
        self.in_flight
    }

    pub(crate) fn complete(&mut self, id: u64) -> Completion {
        if self.in_flight != Some(id) {
            return Completion::Ignored;
        }
        self.in_flight = None;
        if id == self.generation {
            Completion::Current
        } else {
            Completion::Stale
        }
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.in_flight.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidated_in_flight_result_is_stale_and_releases_busy() {
        let mut state = FitState::default();
        let request = state.begin().unwrap();
        state.invalidate();
        assert!(state.is_busy());
        assert_eq!(state.complete(request), Completion::Stale);
        assert!(!state.is_busy());
    }

    #[test]
    fn prevents_a_second_request_while_busy() {
        let mut state = FitState::default();
        let request = state.begin().unwrap();
        assert!(state.is_busy());
        assert_eq!(state.begin(), None);
        assert_eq!(state.complete(request), Completion::Current);
    }

    #[test]
    fn current_matching_completion_releases_busy() {
        let mut state = FitState::default();
        let request = state.begin().unwrap();
        assert_eq!(state.complete(request), Completion::Current);
        assert!(!state.is_busy());
    }

    #[test]
    fn unrelated_completion_cannot_end_another_request() {
        let mut state = FitState::default();
        let request = state.begin().unwrap();
        assert_eq!(state.complete(request.wrapping_add(1)), Completion::Ignored);
        assert!(state.is_busy());
        assert_eq!(state.complete(request), Completion::Current);
    }

    #[test]
    fn failed_send_can_complete_then_retry_with_a_new_id() {
        let mut state = FitState::default();
        let failed_send = state.begin().unwrap();
        assert_eq!(state.complete(failed_send), Completion::Current);
        assert!(!state.is_busy());
        let retry = state.begin().unwrap();
        assert_ne!(retry, failed_send);
        assert_eq!(state.complete(retry), Completion::Current);
    }

    #[test]
    fn invalidation_between_requests_keeps_old_completion_stale() {
        let mut state = FitState::default();
        let old = state.begin().unwrap();
        state.invalidate();
        assert_eq!(state.complete(old), Completion::Stale);
        let current = state.begin().unwrap();
        assert_ne!(current, old);
        assert_eq!(state.complete(current), Completion::Current);
    }
}
