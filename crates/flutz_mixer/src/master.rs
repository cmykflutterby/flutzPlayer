use crate::effects::LimiterControls;

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MasterControls {
    pub volume_db: f64,
    pub limiter: LimiterControls,
    pub reverb: f64,
    pub chorus: f64,
    pub eq_low_db: f64,
    pub eq_mid_db: f64,
    pub eq_high_db: f64,
}

impl Default for MasterControls {
    fn default() -> Self {
        Self {
            volume_db: 0.0,
            limiter: LimiterControls::default(),
            reverb: 0.0,
            chorus: 0.0,
            eq_low_db: 0.0,
            eq_mid_db: 0.0,
            eq_high_db: 0.0,
        }
    }
}
