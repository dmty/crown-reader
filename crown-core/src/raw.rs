#[derive(Debug, Clone, PartialEq)]
pub struct RawSample {
    pub timestamp: u64,
    pub marker: u16,
    pub data: Vec<f64>,
}
