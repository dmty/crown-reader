use std::collections::VecDeque;

/// A bounded most-recent-samples buffer for one channel.
pub struct ChannelRing {
    cap: usize,
    data: VecDeque<f32>,
}

impl ChannelRing {
    pub fn new(cap: usize) -> Self {
        Self { cap, data: VecDeque::with_capacity(cap) }
    }

    pub fn push(&mut self, v: f32) {
        if self.data.len() == self.cap {
            self.data.pop_front();
        }
        self.data.push_back(v);
    }

    pub fn to_vec(&self) -> Vec<f32> {
        self.data.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_most_recent_samples() {
        let mut r = ChannelRing::new(3);
        for v in [1.0, 2.0, 3.0, 4.0] {
            r.push(v);
        }
        assert_eq!(r.to_vec(), vec![2.0, 3.0, 4.0]);
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn starts_empty() {
        assert_eq!(ChannelRing::new(8).to_vec(), Vec::<f32>::new());
    }
}
