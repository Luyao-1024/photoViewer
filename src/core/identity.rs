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
mod tests;
