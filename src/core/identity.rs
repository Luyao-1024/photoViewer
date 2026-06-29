#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MediaId(i64);

impl MediaId {
    pub fn new(value: i64) -> Self {
        Self(value)
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

impl From<i64> for MediaId {
    fn from(value: i64) -> Self {
        Self::new(value)
    }
}

impl From<MediaId> for i64 {
    fn from(value: MediaId) -> Self {
        value.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_id_round_trips_i64() {
        let id = MediaId::from(42_i64);
        assert_eq!(id.get(), 42);
        assert_eq!(i64::from(id), 42);
    }
}
