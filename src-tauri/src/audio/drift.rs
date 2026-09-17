//! Clock drift compensation for the monitors.
//!
//! OBS has none. A monitor is written at the pace of the captured device
//! and read at the pace of the played one, and two crystals never agree
//! exactly: tens of ppm apart, the played device's queue fills or empties
//! by a few milliseconds a minute. OBS lives with it because a monitor
//! rarely runs for hours; a mirror does, and the backends can only throw
//! audio away once the queue is full (`wasapi.rs` skips a packet, `pulse.rs`
//! and `coreaudio.rs` cut the backlog) or play silence once it is empty.
//! Both are audible.
//!
//! [`DriftControl`] is the usual fix: it watches how much audio is queued
//! ahead of the device and slows down or speeds up the monitor's resampler
//! by a few ppm (`swr_set_compensation`) to keep that amount where it was
//! when the output started. A proportional-integral loop, so the integral
//! ends up holding the drift itself and the queue comes back to its level.
//! A thousand ppm is under two cents of pitch, far below what anyone hears.
//!
//! The level has to come from the device's clock (frames written minus
//! frames played), not from how full a buffer looks when a packet arrives:
//! packets and device periods follow two clocks, so a buffer read at packet
//! time jumps by a whole period as their phases slide, and on Windows the
//! mixer empties the stream buffer as soon as it is written anyway.
//!
//! Time is counted in audio written, not on a wall clock, so the loop sees
//! the packets exactly as the device does and can be simulated in tests.

/// Audio ignored at the start, while the device fills its first buffers.
const SETTLE_S: f64 = 2.0;
/// Audio averaged before the level reached is taken as the one to hold.
const LOCK_S: f64 = 3.0;
/// Span over which the level is averaged, and the loop updated.
const WINDOW_S: f64 = 1.0;
/// Time constant of the filter on those averages.
const FILTER_S: f64 = 3.0;
/// A level that moves this much from one packet to the next did not drift:
/// the source paused, or the device skipped or played silence. The level
/// reached afterwards is held instead, and the drift estimate is kept.
const JUMP_S: f64 = 0.040;
/// Proportional gain, per second: a millisecond too much is worked off at
/// 67 ppm.
const KP: f64 = 1.0 / 15.0;
/// Integral gain, per second squared: critically damped with [`KP`], so a
/// drift is absorbed in about two minutes with no overshoot to speak of.
const KI: f64 = KP * KP / 4.0;
/// Largest correction. Real devices stay within a few hundred ppm; past
/// this something else is wrong and the backend's own limits take over.
pub const MAX_PPM: f64 = 1000.0;
/// How often the correction is written to the log, in seconds of audio.
const REPORT_S: f64 = 600.0;

pub struct DriftControl {
    rate: f64,
    elapsed: f64,
    /// Audio seen since the level was last (re)taken.
    since_lock: f64,
    last: Option<f64>,
    /// Sum of the levels of the current window, in seconds, their number,
    /// and the window's length.
    window_sum: f64,
    window_count: u32,
    window: f64,
    /// Filtered average level, in seconds.
    level: f64,
    /// Level held, in seconds, once locked.
    target: Option<f64>,
    integral: f64,
    ppm: f64,
    next_report: f64,
    /// Measure only, never correct. For the hardware test.
    #[cfg(test)]
    pub open_loop: bool,
}

impl DriftControl {
    /// `rate` is the sample rate of the audio written to the device.
    pub fn new(rate: u32) -> Self {
        Self {
            rate: rate.max(1) as f64,
            elapsed: 0.0,
            since_lock: 0.0,
            last: None,
            window_sum: 0.0,
            window_count: 0,
            window: 0.0,
            level: 0.0,
            target: None,
            integral: 0.0,
            ppm: 0.0,
            next_report: REPORT_S,
            #[cfg(test)]
            open_loop: false,
        }
    }

    /// Takes the number of frames queued ahead of the device, read on the
    /// device's clock just before `written` more frames are handed to it,
    /// and returns the correction to apply to the resampler, in ppm of
    /// output rate: above zero plays more frames than the source gives.
    pub fn update(&mut self, queued: f64, written: u32) -> f64 {
        let dt = written as f64 / self.rate;
        self.elapsed += dt;
        if self.elapsed < SETTLE_S {
            return self.ppm;
        }
        let level = queued / self.rate;
        if let Some(last) = self.last.replace(level) {
            if (level - last).abs() > JUMP_S {
                log::debug!("queue level jumped by {:.0} ms", (level - last) * 1000.0);
                self.relock();
                return self.ppm;
            }
        }

        self.since_lock += dt;
        self.window_sum += level;
        self.window_count += 1;
        self.window += dt;
        if self.window < WINDOW_S {
            return self.ppm;
        }
        let dt = std::mem::take(&mut self.window);
        let mean = std::mem::take(&mut self.window_sum)
            / f64::from(std::mem::take(&mut self.window_count));
        let smoothing = 1.0 - (-dt / FILTER_S).exp();

        let Some(target) = self.target else {
            // The first window starts the filter where the level is.
            if self.since_lock < 1.5 * WINDOW_S {
                self.level = mean;
            }
            self.level += (mean - self.level) * smoothing;
            if self.since_lock >= LOCK_S {
                self.target = Some(self.level);
            }
            return self.ppm;
        };
        self.level += (mean - self.level) * smoothing;

        let error = self.level - target;
        // The integral is held where it alone would saturate, so a long
        // stretch against the limit does not have to be unwound afterwards.
        let limit = MAX_PPM / 1e6 / KI;
        self.integral = (self.integral + error * dt).clamp(-limit, limit);
        let ppm = -(KP * error + KI * self.integral) * 1e6;
        #[cfg(test)]
        if self.open_loop {
            return 0.0;
        }
        self.ppm = ppm.clamp(-MAX_PPM, MAX_PPM);
        self.ppm
    }

    /// Takes the level again from the next packets. The integral, which
    /// holds the drift, is kept, and until then so is the correction it
    /// gives on its own.
    fn relock(&mut self) {
        self.target = None;
        self.since_lock = 0.0;
        self.window_sum = 0.0;
        self.window_count = 0;
        self.window = 0.0;
        #[cfg(test)]
        if self.open_loop {
            return;
        }
        self.ppm = (-KI * self.integral * 1e6).clamp(-MAX_PPM, MAX_PPM);
    }

    /// The correction and the distance to the level held, in milliseconds,
    /// once every [`REPORT_S`] of audio, for the log.
    pub fn report(&mut self) -> Option<(f64, f64)> {
        let target = self.target?;
        if self.elapsed < self.next_report {
            return None;
        }
        self.next_report = self.elapsed + REPORT_S;
        Some((self.ppm, (self.level - target) * 1000.0))
    }

    /// Drift estimated so far, in ppm of correction: what the integral
    /// holds.
    #[cfg(test)]
    pub fn estimate(&self) -> f64 {
        -KI * self.integral * 1e6
    }

    /// Filtered level and the level held, in milliseconds.
    #[cfg(test)]
    pub fn levels(&self) -> (f64, Option<f64>) {
        (self.level * 1000.0, self.target.map(|t| t * 1000.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const PACKET: f64 = 480.0;

    /// A device that takes `period` frames at a time on its own clock,
    /// `drift_ppm` slower than the source when positive, fed 10 ms packets
    /// with some timing noise, the correction applied the way the resampler
    /// applies it. The level is read as the backends read it: frames written
    /// minus frames played, on the device's clock, which keeps running
    /// through silence.
    struct Sim {
        ctl: DriftControl,
        period: f64,
        step: f64,
        written: f64,
        /// Frames the device actually holds.
        buffered: f64,
        /// Chunks started, and when the last one did.
        chunks: f64,
        next_chunk: f64,
        fraction: f64,
        seed: u32,
        underruns: u32,
        levels: Vec<f64>,
    }

    impl Sim {
        fn new(drift_ppm: f64, period: usize) -> Sim {
            Sim {
                ctl: DriftControl::new(RATE),
                period: period as f64,
                step: (period as f64 / PACKET) * (1.0 + drift_ppm / 1e6),
                // One packet ahead, as a stream is once it plays.
                written: PACKET,
                buffered: PACKET,
                chunks: 0.0,
                next_chunk: 0.0,
                fraction: 0.0,
                seed: 12345,
                underruns: 0,
                levels: Vec::new(),
            }
        }

        /// Runs from packet `from` to `to` (10 ms each), writing only when
        /// `play` is set.
        fn run(&mut self, from: usize, to: usize, play: bool) {
            for i in from..to {
                self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let jitter = ((self.seed >> 16) as f64 / 65_536.0 - 0.5) * 0.3;
                let now = i as f64 + jitter;
                while self.next_chunk <= now {
                    if self.buffered < self.period && play {
                        self.underruns += 1;
                    }
                    self.buffered = (self.buffered - self.period).max(0.0);
                    self.chunks += 1.0;
                    self.next_chunk += self.step;
                }
                if !play {
                    continue;
                }
                let started = self.next_chunk - self.step;
                let into = ((now - started) / self.step).clamp(0.0, 1.0);
                let position = (self.chunks - 1.0 + into) * self.period;
                let ppm = self.ctl.update(self.written - position, PACKET as u32);
                let out = PACKET * (1.0 + ppm / 1e6) + self.fraction;
                self.fraction = out.fract();
                self.written += out.floor();
                self.buffered += out.floor();
                self.levels.push(self.buffered);
            }
        }
    }

    fn spread(levels: &[f64]) -> f64 {
        let min = levels.iter().cloned().fold(f64::MAX, f64::min);
        let max = levels.iter().cloned().fold(f64::MIN, f64::max);
        max - min
    }

    #[test]
    fn holds_the_level_against_drift() {
        for drift in [-300.0, -40.0, 0.0, 20.0, 100.0, 300.0] {
            for period in [480, 441, 1024] {
                let mut sim = Sim::new(drift, period);
                sim.run(0, 180_000, true);
                // Uncorrected, 300 ppm moves the queue by half a second
                // over this half hour; here it stays within a period and a
                // packet.
                let tail = &sim.levels[sim.levels.len() - 60_000..];
                assert!(
                    spread(tail) <= period as f64 + PACKET + 48.0,
                    "{drift} ppm, period {period}: spread {}",
                    spread(tail)
                );
                let estimate = sim.ctl.estimate();
                assert!(
                    (estimate + drift).abs() < 2.0,
                    "{drift} ppm, period {period}: estimated {estimate}"
                );
            }
        }
    }

    #[test]
    fn a_steady_level_is_left_alone() {
        let mut sim = Sim::new(0.0, 480);
        sim.run(0, 60_000, true);
        assert!(sim.ctl.ppm.abs() < 10.0, "{}", sim.ctl.ppm);
    }

    #[test]
    fn a_pause_keeps_the_drift_and_takes_the_new_level() {
        let mut sim = Sim::new(80.0, 480);
        sim.run(0, 60_000, true);
        assert!((sim.ctl.estimate() + 80.0).abs() < 2.0);
        // Ten seconds with no packets while the device plays silence.
        sim.run(60_000, 61_000, false);
        sim.run(61_000, 61_010, true);
        let ppm = sim.ctl.ppm;
        assert!((ppm + 80.0).abs() < 5.0, "correction kept: {ppm}");
        sim.run(61_010, 120_000, true);
        let (level, target) = sim.ctl.levels();
        assert!((level - target.unwrap()).abs() < 1.0, "{level} {target:?}");
        assert!((sim.ctl.estimate() + 80.0).abs() < 2.0);
    }

    #[test]
    fn the_correction_is_bounded() {
        let mut ctl = DriftControl::new(RATE);
        for i in 0..100_000 {
            let ppm = ctl.update(i as f64 * 4.0, 480);
            assert!(ppm.abs() <= MAX_PPM);
        }
    }
}
