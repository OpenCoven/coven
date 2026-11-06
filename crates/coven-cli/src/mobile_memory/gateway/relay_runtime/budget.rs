//! One shared installation budget, including legacy and counter listeners.
use super::*;

struct State {
    limit: usize,
    used: usize,
}
pub(super) struct ListenerBudget(Mutex<State>);
pub(super) struct ListenerPermit(Arc<ListenerBudget>);
impl ListenerBudget {
    pub(super) fn new(limit: usize) -> Result<Arc<Self>> {
        anyhow::ensure!(
            limit <= storage::MAX_LISTENERS,
            "listener limit exceeds 512 rooms"
        );
        Ok(Arc::new(Self(Mutex::new(State { limit, used: 0 }))))
    }
    pub(super) fn set_limit(&self, limit: usize) -> Result<()> {
        anyhow::ensure!(
            limit <= storage::MAX_LISTENERS,
            "listener limit exceeds 512 rooms"
        );
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("listener budget unavailable"))?
            .limit = limit;
        Ok(())
    }
    pub(super) fn acquire(self: &Arc<Self>) -> Option<ListenerPermit> {
        let mut state = self.0.lock().ok()?;
        if state.used >= state.limit {
            return None;
        }
        state.used += 1;
        Some(ListenerPermit(Arc::clone(self)))
    }
}
impl Drop for ListenerPermit {
    fn drop(&mut self) {
        let mut state = self.0 .0.lock().unwrap_or_else(|error| error.into_inner());
        state.used -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_endpoints_share_one_512_room_budget_and_release_only_owned_permits() {
        let budget = ListenerBudget::new(512).unwrap();
        let mut permits = Vec::new();
        for _ in 0..4 {
            let endpoint = Arc::clone(&budget);
            for _ in 0..128 {
                permits.push(endpoint.acquire().expect("shared capacity refused early"));
            }
        }
        assert!(budget.acquire().is_none());
        drop(permits.pop());
        assert!(budget.acquire().is_some());
        drop(permits);
        let available: Vec<_> = (0..512).map(|_| budget.acquire().unwrap()).collect();
        assert!(budget.acquire().is_none());
        drop(available);
    }
    #[test]
    fn lowering_the_limit_retains_existing_permits_and_refuses_growth_above_512() {
        assert!(ListenerBudget::new(513).is_err());
        let budget = ListenerBudget::new(4).unwrap();
        let mut permits: Vec<_> = (0..4).map(|_| budget.acquire().unwrap()).collect();
        budget.set_limit(2).unwrap();
        assert!(budget.acquire().is_none());
        drop(permits.pop());
        drop(permits.pop());
        assert!(budget.acquire().is_none());
        drop(permits.pop());
        let retained = budget.acquire().unwrap();
        assert!(budget.acquire().is_none());
        assert!(budget.set_limit(513).is_err());
        assert!(budget.acquire().is_none());
        drop(retained);
        drop(permits);
        budget.set_limit(0).unwrap();
        assert!(budget.acquire().is_none());
    }
}
