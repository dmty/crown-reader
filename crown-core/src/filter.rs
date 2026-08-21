//! Display filtering for raw EEG.
//!
//! The OSC transport delivers the unfiltered signal — measured on hardware,
//! mains sits ~190x above the alpha band, so an unfiltered trace shows only
//! hum. The Bluetooth `raw` characteristic was notch-filtered on-device,
//! which is why it looked clean; nothing filters this one for us.

/// Mains frequency is regional and must never be hardcoded: 50 Hz across
/// most of the world, 60 Hz across the Americas. Getting it wrong leaves the
/// hum untouched and notches out a band the signal actually uses.
#[derive(Debug, Clone, Copy)]
pub struct FilterConfig {
    pub mains_hz: f64,
    pub highpass_hz: f64,
    pub sample_rate_hz: f64,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self { mains_hz: 50.0, highpass_hz: 1.0, sample_rate_hz: 256.0 }
    }
}

/// One channel's filter state: a one-pole high-pass for electrode drift,
/// then a biquad notch for mains.
///
/// Only the mains fundamental is notched. The second harmonic measured at
/// amplitude 32.8 against a 6327 fundamental — small enough that a second
/// biquad would cost state and settling time for no visible gain.
#[derive(Debug, Clone)]
pub struct ChannelFilter {
    highpass_alpha: f64,
    prev_in: f64,
    prev_out: f64,
    notch: Biquad,
}

impl ChannelFilter {
    pub fn new(config: &FilterConfig) -> Self {
        let dt = 1.0 / config.sample_rate_hz;
        let rc = 1.0 / (2.0 * std::f64::consts::PI * config.highpass_hz);
        Self {
            highpass_alpha: rc / (rc + dt),
            prev_in: 0.0,
            prev_out: 0.0,
            notch: Biquad::notch(config.mains_hz, config.sample_rate_hz),
        }
    }

    /// Returns 0.0 for a non-finite input rather than admitting it to the
    /// filter state. An IIR filter feeds its own output back, so one NaN
    /// would otherwise make every subsequent sample NaN for the life of the
    /// session.
    pub fn apply(&mut self, x: f64) -> f64 {
        if !x.is_finite() {
            return 0.0;
        }
        let high = self.highpass_alpha * (self.prev_out + x - self.prev_in);
        self.prev_in = x;
        self.prev_out = high;
        self.notch.apply(high)
    }

    /// Predicts the sample that would have arrived, for use when the
    /// transport drops one.
    ///
    /// A sinusoid at `w0` satisfies `x[n] = 2*cos(w0)*x[n-1] - x[n-2]`
    /// exactly, and what goes missing here is dominated by mains hum, so a
    /// two-tap recursion reconstructs it closely. This is not cosmetic: fed
    /// a stream with gaps simply closed up, the notch below leaves more hum
    /// than applying no filter at all, because every gap is a phase step
    /// that rings its poles and the ringing never fully decays between
    /// drops.
    pub fn predict_next(&self) -> f64 {
        self.notch.two_cos_w0 * self.notch.x1 - self.notch.x2
    }
}

/// Direct-form-I biquad. Coefficients from the RBJ audio EQ cookbook.
#[derive(Debug, Clone)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
    /// `2*cos(w0)`, cached for `ChannelFilter::predict_next`'s two-tap
    /// sinusoid recursion rather than recomputed per prediction.
    two_cos_w0: f64,
}

impl Biquad {
    /// Q of 10, chosen for tolerance to mains drift rather than for maximum
    /// depth at the nominal frequency: Q=30 notches deepest exactly at f0,
    /// but 0.05 Hz of mistuning — grid drift, or the device's true rate
    /// differing from the configured 256 — leaves hum at 15x alpha. Q=10
    /// holds mistuned hum to 5x instead, at a cost of only 1.6% extra bleed
    /// at 40 Hz.
    fn notch(freq_hz: f64, sample_rate_hz: f64) -> Self {
        const Q: f64 = 10.0;
        let w0 = 2.0 * std::f64::consts::PI * freq_hz / sample_rate_hz;
        let (sin_w0, cos_w0) = w0.sin_cos();
        let alpha = sin_w0 / (2.0 * Q);
        let a0 = 1.0 + alpha;
        Self {
            b0: 1.0 / a0,
            b1: -2.0 * cos_w0 / a0,
            b2: 1.0 / a0,
            a1: -2.0 * cos_w0 / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
            two_cos_w0: 2.0 * cos_w0,
        }
    }

    fn apply(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `seconds` of a sine at `hz` through the filter and returns the
    /// peak absolute value over the final third, by which point the filter
    /// has settled. Peak, not RMS: it is what a waveform display shows.
    fn response(filter: &mut ChannelFilter, hz: f64, rate: f64, seconds: f64) -> f64 {
        let n = (rate * seconds) as usize;
        let mut peak: f64 = 0.0;
        for i in 0..n {
            let t = i as f64 / rate;
            let y = filter.apply((2.0 * std::f64::consts::PI * hz * t).sin());
            if i > n * 2 / 3 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn mains_hum_is_attenuated_by_at_least_twenty_decibels() {
        let config = FilterConfig::default();
        let mut filter = ChannelFilter::new(&config);
        let attenuated = response(&mut filter, config.mains_hz, config.sample_rate_hz, 4.0);
        assert!(attenuated < 0.1, "50 Hz peak {attenuated} should be under 0.1");
    }

    #[test]
    fn the_alpha_band_passes_through_largely_intact() {
        let config = FilterConfig::default();
        let mut filter = ChannelFilter::new(&config);
        let passed = response(&mut filter, 10.0, config.sample_rate_hz, 4.0);
        assert!(passed > 0.9, "10 Hz peak {passed} should stay above 0.9");
        assert!(passed < 1.1, "10 Hz peak {passed} should not be amplified above unity");
    }

    #[test]
    fn a_dc_offset_is_removed() {
        let mut filter = ChannelFilter::new(&FilterConfig::default());
        let mut last = 0.0;
        for _ in 0..2048 {
            last = filter.apply(1000.0);
        }
        assert!(last.abs() < 1.0, "DC should settle to ~0, got {last}");
    }

    #[test]
    fn sixty_hertz_mains_is_configurable_not_hardcoded() {
        let config = FilterConfig { mains_hz: 60.0, ..FilterConfig::default() };
        let mut filter = ChannelFilter::new(&config);
        let attenuated = response(&mut filter, 60.0, config.sample_rate_hz, 4.0);
        assert!(attenuated < 0.1, "60 Hz peak {attenuated} should be under 0.1");
        // And the 50 Hz it is no longer tuned to must survive.
        let mut other = ChannelFilter::new(&config);
        let passed = response(&mut other, 50.0, config.sample_rate_hz, 4.0);
        assert!(passed > 0.5, "50 Hz peak {passed} should survive a 60 Hz notch");
    }

    #[test]
    fn a_non_finite_input_does_not_poison_the_filter_state() {
        let mut filter = ChannelFilter::new(&FilterConfig::default());
        for _ in 0..100 {
            filter.apply(10.0);
        }
        assert_eq!(filter.apply(f64::NAN), 0.0);
        // A NaN must not wedge every subsequent sample into NaN.
        let after = filter.apply(10.0);
        assert!(after.is_finite(), "filter poisoned by one NaN: {after}");
    }

    #[test]
    fn the_notch_still_attenuates_when_mistuned_by_a_tenth_of_a_hertz() {
        let config = FilterConfig::default();
        let mut filter = ChannelFilter::new(&config);
        let attenuated =
            response(&mut filter, config.mains_hz + 0.1, config.sample_rate_hz, 4.0);
        assert!(attenuated < 0.1, "mistuned 50.1 Hz peak {attenuated} should still be attenuated");
    }

    #[test]
    fn predict_next_on_a_fresh_filter_is_zero_not_nan() {
        let filter = ChannelFilter::new(&FilterConfig::default());
        assert_eq!(filter.predict_next(), 0.0);
    }

    #[test]
    fn predicting_a_dropped_mains_sample_does_not_ring_the_notch() {
        let config = FilterConfig::default();
        let rate = config.sample_rate_hz;

        let mut clean = ChannelFilter::new(&config);
        let clean_peak = response(&mut clean, config.mains_hz, rate, 4.0);

        let mut filter = ChannelFilter::new(&config);
        let n = (rate * 4.0) as usize;
        let mut peak: f64 = 0.0;
        for i in 0..n {
            let t = i as f64 / rate;
            let y = if i > 0 && i % 50 == 0 {
                // Simulates a dropped packet: no raw sample arrives, so the
                // prediction is fed straight to the notch in place of the
                // high-passed value the missing sample would have produced.
                let predicted = filter.predict_next();
                filter.notch.apply(predicted)
            } else {
                filter.apply((2.0 * std::f64::consts::PI * config.mains_hz * t).sin())
            };
            if i > n * 2 / 3 {
                peak = peak.max(y.abs());
            }
        }
        assert!(
            peak < clean_peak + 0.05,
            "predicted-gap peak {peak} should stay close to the no-loss peak {clean_peak}"
        );
    }
}
