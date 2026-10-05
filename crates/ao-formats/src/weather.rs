//! The client's procedural weather (Gamecode.dll), a deterministic port of the schedule, the weather struct, and the
//! values derived from it (clouds, storm/precipitation, high altitude wind, rain/wind/quake sound levels).
//!
//! # Evidence (Gamecode.dll)
//! * `FUN_100bdb64` (sky manager ctor): `FUN_100bc853` allocates the 0x54 byte *area config* (bytes `0..6` = ambient / fog
//!   colour / fog density, `+0x20..+0x3c` eight `u32` weights, `+0x44..+0x4e` eleven parameter bytes), `FUN_100bd2c4` /
//!   `FUN_100bd183` fill it from `EnvironmentData_t` (bytes `0..7` weights, `8..0x12` parameters, `0x13..0x19` colours) and
//!   `FUN_100bce61` doubles the weights (`w > 1 ⇒ 2w + 100`), sums them into `+0x40` and builds the schedule.
//! * `FUN_100b12c9` (schedule ctor): three 6480 s windows of a 19439.998 s period (`_DAT_101683a0`, `/ 3.0` @0x101602d0),
//!   events spaced `rand · (1200 − 120) + 120` s (`_DAT_101683a8/a4`), per event 16 floats and 8 ints drawn from a generator
//!   re-seeded with the event's own seed. Generator = R250 lagged xor `FUN_1013d8fa/93c/97c` (init table @0x101717b8, `u32 ·
//!   2⁻³²` @0x1016acb0). Window seeds `FUN_100bc8d1(−1, 0, +1)` = Σ config bytes + Σ weights + game day + n (0 ⇒ 1).
//! * `FUN_100b0d1f` (advance), `FUN_100b0f43/fa1` (select previous / current event), `FUN_100b0eaa` (wrapped difference),
//!   `FUN_100b0fff` (elapsed), `FUN_100b0e34/e5e/e9d` (draw float / int / event length).
//! * `FUN_100bcfff` (per frame): rebuilds the schedule when the game day changes, counts the two timers down by 1 per call
//!   (`+0x14`, `+0x18`), recomputes weather slots 0 (previous event) and 1 (current event) with `FUN_100bc970` and blends
//!   them with `FUN_100bf417` by the ramp factor `FUN_100bc970` returns.
//! * `FUN_100bc970`: fills the weather struct (see [`Fields`]).
//! * `FUN_100be767` (sky manager frame): copies the struct, writes `ThickCloudsIntensity = f[2]` (`FUN_100ad5db`,
//!   DisplaySystem `+0x114`), `CloudTranspariency = 1 − f[2] / 2.33333` (`+0x1134`, double @0x101684d0), `GameStormIntensity =
//!   max f[3..=6]` (`FUN_100bdb31`, `+0xf0`), `GamePrecipitationIntensity = f[0]` (`FUN_100bd62e`, `+0xf4`) and
//!   `HighAltitudeWind = 0.004 · 0.5 · dt · (C + D)` (`FUN_100ad568`, `+0xd8/+0xe0`), see [`Wind`].
//! * `FUN_100b6d67` (music, once per second): storm (`max f[3..=6] > 0.4`) → slot 7, rain (`f[0]`) → 6, fog (`f[1]`) → 5.
//!
//! # Not reproduced (UNRESOLVED)
//! * The game day comes from the server (`GameTime_t::Update(.., day, ..)` @0x1000b526, `GetCurrentDay` +0x4c); offline the
//!   caller supplies it ([`OFFLINE_DAY`]).
//! * The C runtime `rand()` (wind walk `FUN_100bf729`, quake `FUN_100be767`, rain `FUN_100b964d`) is shared with everything
//!   else in the process and never seeded in the code searched (`srand` has no caller in Gamecode); we use the MSVC LCG
//!   seeded with 1 for wind and quake only.
//! * Per-area weather (`FUN_100be576` blends the weather of areas by camera distance; `PlayfieldAnarchy +0xb4` vector, area
//!   `+0x48..+0x50` position, `+0x58` radius) needs the area records, which are not decoded: only the playfield-level
//!   `EnvironmentData` drives this port.
//! * Lightning: `FUN_100b9850` runs two more schedules (`FUN_100b15e8`, seeds 0x85c19 / 0x1e23a, period 600 / 300 s) whose
//!   strike positions come from the camera position and the tilemap size (`FUN_100b1622` not decoded); `SM_Sandy_Env_
//!   NearLightning` plays only for strikes within 25 m. Only the enabling condition is ported ([`State::lightning_enabled`]).
//! * `f[10]` (+0x28), `f[11]` (+0x2c), `f[13]` (+0x34): computed by `FUN_100bc970`, consumer not found.

/// Game day used when no server day is known (the game day only seeds the schedule).
pub const OFFLINE_DAY: u32 = 0;

/// Number of floats of the weather struct that `FUN_100bf417` blends / `FUN_100bc970` fills.
pub const FIELDS: usize = 22;

/// The weather struct (`FUN_100bc970`), float index = byte offset / 4.
///
/// | idx | meaning |
/// |-----|---------|
/// | 0 | precipitation (rain), weight band `w0` (`GamePrecipitationIntensity`) |
/// | 1 | fog, band `w1` (`draw · 0.65`) |
/// | 2 | thick cloud opacity (`ThickCloudsIntensity`), band `w2`; afterwards max of f[0], f[1], f[4], f[14] |
/// | 3 | sand storm flag/ramp (band `w4`; `SM_Sandy_Env_SandWind`) |
/// | 4 | ash storm (band `w5`) [name INFERENCE: `Env - Setting ash storm weather` debug order] |
/// | 5 | fallout red (band `w6`; `SM_Sandy_Env_FalloutRWind`) |
/// | 6 | fallout green (band `w7`; `SM_Sandy_Env_FalloutGWind`) |
/// | 7 | wind speed, m/s (`(draw · (b9 − b8) · ramp + b8) · 2.75`, or `33 · f[14]`) |
/// | 8 | rain drop parameter (`draw · (b10 − b11) · ramp + b11`) |
/// | 9 | lightning level (only when `f[2] > 0.3`, probability `b12` %) |
/// | 10, 11, 12, 13 | `b13` %-gated, `b14/b15`, `b16/b17` gated (12 = quake rate), `b18` |
/// | 14 | first positive of f[3], f[5], f[6] (**persistent** across fills when none is positive) |
/// | 15..=17 | ambient colour `b0..b2 / 255` |
/// | 18..=20 | fog colour `b3..b5 / 255` |
/// | 21 | fog density `b6 / 100` |
pub type Fields = [f32; FIELDS];

// -- R250 ---------------------------------------------------------------------------------------------------------------

/// Initial state of the generator, Gamecode @0x101717b8 (250 words).
const R250_INIT: [u32; 250] = [
    0xf6230029, 0x26e16784, 0x20ae2cd6, 0xe7495f90, 0xa8bb5af1, 0xadb301eb,
    0x923c12db, 0x963e390c, 0x3d5e0124, 0xfa06491c, 0xe7de1547, 0xcf4d2d12,
    0xedbb6443, 0x731f26a6, 0x7f7d7a5a, 0x24251238, 0xd1d46e5d, 0x8a966bfc,
    0x213b4e45, 0x6189260d, 0xa5db301c, 0x9b200732, 0x62ee2350, 0x96365878,
    0x28493e12, 0x479e3bf6, 0xa0dc5f49, 0xac14314f, 0x48404944, 0xf26b1cd0,
    0x4cb74230, 0x37a12c3b, 0x99223ef6, 0x99e1409d, 0xc3da121f, 0x349926ca,
    0xca727bb9, 0xe92c7049, 0x8fc5187e, 0x93e93cd5, 0xfaea5db2, 0xe85348cc,
    0x83d65c67, 0x14d62f14, 0xdadc422d, 0x96830d66, 0x4d494657, 0xe3692fff,
    0xf3cd3a61, 0xc29d261e, 0x13721916, 0xb01d32e6, 0x354f0384, 0x93020677,
    0x7c396be8, 0xa2cb1953, 0x6c1e0e12, 0x289e7874, 0x89d511f4, 0xe8d45a9f,
    0x277e2059, 0x453207cf, 0xc1cc1af4, 0x239001d3, 0xd8d36048, 0xeee60975,
    0xc02a591d, 0x62f71dc0, 0x93815078, 0x060e7b44, 0xff001850, 0x848d7f61,
    0x45050c7b, 0x443b3807, 0x3d1f7282, 0xee926270, 0x83545064, 0x72853bb1,
    0x92156d69, 0x4c6a5c46, 0x50731796, 0x301673d9, 0x8d684d67, 0x754a2cf7,
    0x26575ed0, 0x70fa5876, 0xc21149bb, 0xd5244eae, 0x2efe5579, 0x642548db,
    0x9b3c0de5, 0xcbd35f45, 0x02ce0a28, 0x25c568f5, 0xb13d3459, 0x4a8a4027,
    0xf82d5e76, 0xa2c97ac2, 0x31d42668, 0x4179086a, 0x3a614e08, 0x68b17014,
    0x51a50d6a, 0xf9c12528, 0x05a954d6, 0xb3973087, 0x25f1412f, 0xd89a441d,
    0xbb9b00c1, 0x877e4fc0, 0x989a5fa8, 0xf3c26486, 0xcabf7a54, 0x33d92fe7,
    0xde5579d1, 0xd6282a38, 0xcd6c10d9, 0x125e4c66, 0x8c3001e1, 0x6ad94efe,
    0xeae2159f, 0xb40c28e2, 0x024766b4, 0x202a4e38, 0xd2a91289, 0x677a2079,
    0x55c20878, 0x992626b1, 0x632927da, 0xc462113e, 0x51127296, 0x88394ebf,
    0x30891d3f, 0x4b6d1ff1, 0x226c06e3, 0x2c1e36a1, 0xdfcb721d, 0x363f1003,
    0x08840607, 0x56057514, 0x0a28791b, 0xbfc56bc9, 0x468e212c, 0xef087a36,
    0xada84af3, 0xc5be78fe, 0x877100eb, 0x72d064a0, 0xbd061c75, 0x7287357e,
    0xd84d6d73, 0xe68254be, 0x11c243db, 0xd12b5841, 0x103003fa, 0xadf05a70,
    0x35867954, 0x26aa1295, 0x7d132568, 0xeefc58e6, 0x3fbe1eca, 0xf9890d9f,
    0x991b0a41, 0x0b4f7cb8, 0x41721af6, 0xc3996014, 0x8f0d27d3, 0xac3a2044,
    0x1c6613a6, 0xd20b7833, 0x0a1052a1, 0x76ec3a4c, 0xb773134c, 0x49b43605,
    0xe9aa4a0e, 0x9cd532cf, 0xe62765ca, 0x5f9d31d8, 0x6af4194d, 0x0f593a27,
    0x969c387c, 0xc7cd6af8, 0x53207987, 0xb1b87e64, 0x74954987, 0x8b5a5ab0,
    0xad2d214e, 0xec3d5ae7, 0x83de3260, 0x39ad2780, 0x3ae92d41, 0x8ba55e41,
    0x3ae4199f, 0x6d15749f, 0x8a260e00, 0x72d1424c, 0x24935804, 0x708f09b3,
    0x1a402753, 0x76af328a, 0xcc6f5cca, 0xf2d34ecf, 0x1fce0c95, 0x1c657cbe,
    0x44f65d2a, 0x91a158ad, 0xd3a60665, 0x4ec007c9, 0x20341b32, 0x394d012c,
    0x37174d8f, 0xf6db2cc6, 0xa7a64fc8, 0xef88480b, 0x4f612738, 0x50124e68,
    0xea5403f9, 0x247177e7, 0x6a784e48, 0x51294a92, 0xa2296586, 0x3cdb4b72,
    0x64fa561c, 0x9eb82237, 0x789562b0, 0xdcdf0ef5, 0x9a6d726c, 0xe8d97374,
    0x62651943, 0x9aa57a08, 0xe0bd5079, 0x981521eb, 0x75e164e0, 0x366e3d8f,
    0x54f46d7b, 0xb68a4962, 0x0127188f, 0xd5e501f7, 0x3b3b164a, 0x2b61569b,
    0xa0c62f84, 0x2da751b1, 0xca107c4a, 0x106f6275, 0x469e012f, 0x65102bef,
    0xc4c80b9b, 0x7c292b43, 0xa1f6448a, 0x80473b9e,
];

/// `FUN_1013d8fa` (seed), `FUN_1013d93c` (next word), `FUN_1013d97c` (next word as fraction of 2³²).
#[derive(Clone)]
pub struct R250 {
    s: [u32; 250],
    i: usize,
}

impl R250 {
    pub fn new(seed: u32) -> R250 {
        let mut s = R250_INIT;
        let i = ((seed & 0xff) as usize).min(0xf9);
        s[i] = s[i].wrapping_add(seed);
        R250 { s, i }
    }

    pub fn next_u32(&mut self) -> u32 {
        let i = self.i;
        let j = if i < 0x93 { i + 0x67 } else { i - 0x93 };
        let v = self.s[j] ^ self.s[i];
        self.s[i] = v;
        self.i = if i < 0xf9 { i + 1 } else { 0 };
        v
    }

    pub fn next_f64(&mut self) -> f64 {
        self.next_u32() as f64 * (1.0 / 4294967296.0)
    }
}

/// The MSVC C runtime `rand()` (default seed 1).
#[derive(Clone)]
pub struct CrtRand(u32);

impl Default for CrtRand {
    fn default() -> Self {
        CrtRand(1)
    }
}

impl CrtRand {
    pub fn rand(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(214013).wrapping_add(2531011);
        ((self.0 >> 16) & 0x7fff) as i32
    }
}

// -- schedule -----------------------------------------------------------------------------------------------------------

/// `_DAT_101683a0` (`0x4697DFFF`), `_DAT_101683a4`, `_DAT_101683a8`.
const PERIOD: f32 = f32::from_bits(0x4697_DFFF);
const GAP_MIN: f32 = 120.0;
const GAP_MAX: f32 = 1200.0;
/// `GameTime_t +0x5c`: game seconds per day-time second.
const TIME_DIV: f64 = 15.0;

struct Event {
    time: f32,
    seed: u32,
    floats: [f32; 16],
    ints: [u32; 8],
    fi: usize,
    ii: usize,
}

/// `FUN_100b12c9` and its runtime state (`FUN_100b0d1f` ...).
pub struct Schedule {
    period: f32,
    window: f32,
    events: Vec<Event>,
    cursor: usize,
    active: usize,
    last: f32,
    fired: usize,
    started: bool,
    next: usize,
    prev: usize,
    ev_time: f32,
    ev_len: f32,
}

impl Schedule {
    /// `FUN_100b12c9(seed0, seed1, seed2, period, gap_min, gap_max, 16, 8)`.
    pub fn new(seeds: [u32; 3], period_arg: f32, gap_min: f32, gap_max: f32) -> Schedule {
        let window = (period_arg as f64 / 3.0) as f32;
        let period = if (period_arg as f64) < 0.1 { 0.1 } else { period_arg };
        let mut events: Vec<Event> = Vec::new();
        if gap_min < gap_max {
            for (n, &seed) in seeds.iter().enumerate() {
                let mut rng = R250::new(seed);
                let first = events.len();
                if window > 0.0 {
                    let mut t = 0f32;
                    loop {
                        t = (rng.next_f64() * (gap_max - gap_min) as f64 + gap_min as f64 + t as f64) as f32;
                        if t < window {
                            events.push(Event { time: (n as f64 * window as f64 + t as f64) as f32, seed: 0, floats: [0.0; 16], ints: [0; 8], fi: 0, ii: 0 });
                        } else {
                            break;
                        }
                    }
                }
                for e in &mut events[first..] {
                    e.seed = rng.next_u32();
                }
                for e in &mut events[first..] {
                    rng = R250::new(e.seed);
                    for f in &mut e.floats {
                        *f = rng.next_f64() as f32;
                    }
                    for i in &mut e.ints {
                        *i = rng.next_u32();
                    }
                }
            }
        }
        Schedule { period, window, events, cursor: 0, active: 0, last: 0.0, fired: 0, started: false, next: 0, prev: 0, ev_time: 0.0, ev_len: 1.0 }
    }

    /// Number of events in the whole period.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Event start times (schedule time, 0..period).
    pub fn times(&self) -> impl Iterator<Item = f32> + '_ {
        self.events.iter().map(|e| e.time)
    }

    /// Schedule time of a day time (`GameDayTime`): `fmod(realtime / 15 + window, period)` (`FUN_100b0d1f`, `FUN_100b0fff`).
    pub fn time_of(&self, day_time: f64) -> f32 {
        let rt = (day_time * TIME_DIV) as f32;
        let x = (rt as f64 / TIME_DIV + self.window as f64) as f32;
        ((x as f64) % (self.period as f64)) as f32
    }

    /// `FUN_100b0eaa`.
    fn wrapped_diff(&self, a: f32, b: f32) -> f32 {
        let a = if a < b { a + self.period } else { a };
        a - b
    }

    /// `FUN_100b0d1f`: one step of the event cursor; `true` while an event fired.
    fn step(&mut self, t: f32) -> bool {
        let n = self.events.len();
        if n != 0 && self.fired < n {
            if self.started {
                self.cursor += 1;
                if self.cursor == n {
                    self.cursor = 0;
                }
            }
            let e = self.events[self.cursor].time;
            let fire = if self.last <= t { self.last < e && e <= t } else { e <= t || self.last < e };
            if fire {
                self.fired += 1;
                self.active = self.cursor;
                self.events[self.cursor].fi = 0;
                self.events[self.cursor].ii = 0;
                self.next = self.cursor;
                self.started = true;
                self.prev = if self.cursor == 0 { n - 1 } else { self.cursor - 1 };
                return true;
            }
        }
        self.last = t;
        self.fired = 0;
        self.started = false;
        false
    }

    /// `FUN_100b0f33`.
    fn advance(&mut self, t: f32) {
        while self.step(t) {}
    }

    /// `FUN_100b0f43`: slot 0 = the event before the last fired one.
    fn select_previous(&mut self) {
        if self.events.is_empty() {
            return;
        }
        self.active = self.prev;
        self.reset_draws();
        self.ev_time = self.events[self.prev].time;
        self.ev_len = self.wrapped_diff(self.events[self.next].time, self.ev_time);
    }

    /// `FUN_100b0fa1`: slot 1 = the last fired event, length up to the next candidate.
    fn select_current(&mut self) {
        if self.events.is_empty() {
            return;
        }
        self.active = self.next;
        self.reset_draws();
        self.ev_time = self.events[self.next].time;
        self.ev_len = self.wrapped_diff(self.events[self.cursor].time, self.ev_time);
    }

    fn reset_draws(&mut self) {
        let e = &mut self.events[self.active];
        e.fi = 0;
        e.ii = 0;
    }

    /// `FUN_100b0e34`.
    fn draw_f(&mut self) -> f32 {
        match self.events.get_mut(self.active) {
            Some(e) => {
                e.fi = (e.fi + 1) & 15;
                e.floats[e.fi]
            }
            None => 0.0,
        }
    }

    /// `FUN_100b0e5e`.
    fn draw_i(&mut self) -> u32 {
        match self.events.get_mut(self.active) {
            Some(e) => {
                e.ii = (e.ii + 1) & 7;
                e.ints[e.ii]
            }
            None => 0,
        }
    }

    /// `FUN_100b0e9d`.
    fn length(&self) -> f32 {
        if self.events.is_empty() { 1.0 } else { self.ev_len }
    }
}

// -- area config + weather struct ---------------------------------------------------------------------------------------

/// The 0x54 byte area config (`FUN_100bc853`, filled by `FUN_100bd2c4`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// `b[0..=6]`: ambient rgb, fog rgb, fog density % (`EnvironmentData` bytes 0x13..=0x19).
    pub colors: [u8; 7],
    /// `+0x20..+0x3c` after `FUN_100bce61`: rain, fog, cloud, clear, sand, ash, fallout R, fallout G.
    pub weights: [u32; 8],
    /// `+0x44..+0x4e` = `EnvironmentData` bytes 8..=0x12.
    pub params: [u8; 11],
}

impl Config {
    /// From the 26 raw `EnvironmentData_t` bytes in stream order (`playfield::environment::Env::raw`; stream order swaps
    /// bytes 0xa and 0xb).
    pub fn from_env(raw: &[u8; 26]) -> Config {
        let idx = |i: usize| match i {
            0xa => raw[11],
            0xb => raw[10],
            _ => raw[i],
        };
        let weights = std::array::from_fn(|i| {
            let w = raw[i] as u32;
            if w > 1 { w * 2 + 100 } else { w }
        });
        Config { colors: std::array::from_fn(|i| raw[0x13 + i]), weights, params: std::array::from_fn(|i| idx(8 + i)) }
    }

    /// `+0x40`: weights sum; 0 means no weather ever.
    pub fn total(&self) -> u32 {
        self.weights.iter().sum()
    }

    /// `FUN_100bc8d1`.
    fn seed(&self, day: u32, n: u32) -> u32 {
        let sum = self.colors.iter().chain(&self.params).map(|&b| b as u32).chain(self.weights.iter().copied()).fold(0u32, u32::wrapping_add);
        let s = sum.wrapping_add(day).wrapping_add(n);
        if s == 0 { 1 } else { s }
    }

    fn schedule(&self, day: u32) -> Schedule {
        Schedule::new([self.seed(day, u32::MAX), self.seed(day, 0), self.seed(day, 1)], PERIOD, GAP_MIN, GAP_MAX)
    }
}

/// `FUN_100bf417`: `dst = t · src + (1 − t) · dst`, colour fields weighted by the fog density (index 21).
fn blend(dst: &mut Fields, src: &Fields, t: f32) {
    if t <= 0.0 || t.is_nan() {
        return;
    }
    if t >= 1.0 {
        *dst = *src;
        return;
    }
    let u = 1.0 - t;
    for i in 0..18 {
        dst[i] = (t as f64 * src[i] as f64 + u as f64 * dst[i] as f64) as f32;
    }
    let w = dst[21] * u;
    let total = t * src[21] + w;
    let k = if 0.0 < total { w / total } else { 0.0 };
    let k1 = 1.0 - k;
    for i in 18..21 {
        dst[i] = src[i] * k1 + k * dst[i];
    }
    dst[21] = t * src[21] + u * dst[21];
}

/// One area's weather (`FUN_100bcfff` + `FUN_100bc970`).
pub struct Area {
    cfg: Config,
    sched: Schedule,
    seed_day: u32,
    timers: [f32; 2],
    ramp: f32,
    slots: [Fields; 2],
    now: f32,
}

impl Area {
    pub fn new(cfg: Config, day: u32) -> Area {
        let sched = cfg.schedule(day);
        Area { cfg, sched, seed_day: day, timers: [0.0; 2], ramp: 0.0, slots: [[0.0; FIELDS]; 2], now: 0.0 }
    }

    /// The blended current struct (`this + 0xc`).
    pub fn fields(&self) -> &Fields {
        &self.slots[0]
    }

    /// `FUN_100bcfff`. `dt` is `GameTime +0x78`, `day_time` the `GameDayTime`.
    pub fn update(&mut self, day: u32, day_time: f64, dt: f32) {
        if self.cfg.total() == 0 {
            return;
        }
        if self.seed_day != day {
            self.sched = self.cfg.schedule(day);
            self.seed_day = day;
            self.timers = [0.0; 2];
        }
        // the client subtracts `1.0` unless the frame time equals the day number (`FUN_100bcfff` @0x100bd0b3)
        let step = if dt == day as f32 { 0.0 } else { 1.0 };
        self.timers[0] -= step;
        self.timers[1] -= step;
        if self.timers[0] > 0.0 && self.timers[1] > 0.0 {
            return;
        }
        self.now = self.sched.time_of(day_time);
        self.sched.advance(self.now);
        if self.timers[0] <= 0.0 {
            self.sched.select_previous();
            self.fill(0);
        }
        if self.timers[1] <= 0.0 {
            self.sched.select_current();
            self.ramp = self.fill(1);
        }
        let (a, b) = self.slots.split_at_mut(1);
        blend(&mut a[0], &b[0], self.ramp);
    }

    /// `FUN_100bc970(slot)`; returns the ramp factor.
    fn fill(&mut self, slot: usize) -> f32 {
        let total = self.cfg.total();
        let w = self.cfg.weights;
        let b = self.cfg.params;
        let elapsed = self.sched.wrapped_diff(self.now, self.sched.ev_time);
        let mut mult = 1f32;
        let mut f = self.slots[slot];
        for v in &mut f[..7] {
            *v = 0.0;
        }
        let mut u = self.sched.draw_i() % total;
        if u >= w[3] {
            u -= w[3];
            if u < w[0] {
                f[0] = self.sched.draw_f();
            } else {
                u -= w[0];
                if u < w[1] {
                    f[1] = (self.sched.draw_f() as f64 * 0.65) as f32;
                } else {
                    u -= w[1];
                    if u < w[2] {
                        f[2] = self.sched.draw_f();
                    } else {
                        u -= w[2];
                        for (k, i) in [(4, 3), (5, 4), (6, 5), (7, 6)] {
                            if u < w[k] {
                                f[i] = 1.0;
                                break;
                            }
                            u -= w[k];
                        }
                        // `local_c = 0.5` is set for the four flag bands only (the last band's miss keeps 1.0)
                        if f[3..7].contains(&1.0) {
                            mult = 0.5;
                        }
                    }
                }
            }
        }
        let len = self.sched.length();
        let rr = self.sched.draw_f();
        let dur = ((len as f64 * 0.25 + rr as f64 * len as f64 * 0.75) * mult as f64) as f32;
        let scale;
        if dur < elapsed {
            scale = 1.0f32;
            if slot != 0 {
                self.timers[1] = len - elapsed;
                self.timers[0] = len - elapsed;
            }
        } else if 0.0 < elapsed {
            scale = elapsed / dur;
            if slot != 0 {
                self.timers[1] = (dur as f64 / 255.0) as f32;
                self.timers[0] = len - elapsed;
            }
        } else {
            scale = 0.0;
            if slot != 0 {
                self.timers[0] = len - elapsed;
            }
        }
        for v in &mut f[..7] {
            *v *= scale;
        }
        let range = |s: &mut Schedule, lo: u8, hi: u8| (s.draw_f() as f64 * (hi as i32 - lo as i32) as f64 * scale as f64 + lo as f64) as f32;
        f[7] = range(&mut self.sched, b[0], b[1]);
        f[7] = (f[7] as f64 * 2.75) as f32;
        if 0.0 < f[3] {
            f[14] = f[3];
        } else if 0.0 < f[5] {
            f[14] = f[5];
        } else if 0.0 < f[6] {
            f[14] = f[6];
        }
        if 0.0 < f[14] {
            f[7] = (f[14] as f64 * 33.0) as f32;
        }
        for i in [14, 4, 0, 1, 4] {
            if f[2] < f[i] {
                f[2] = f[i];
            }
        }
        f[8] = range(&mut self.sched, b[3], b[2]);
        let gated = |s: &mut Schedule, cloud: f32, prob: u8, amount: Option<u8>| -> f32 {
            if cloud <= 0.3 {
                return 0.0;
            }
            gate(s, prob, amount, scale)
        };
        f[9] = gated(&mut self.sched, f[2], b[4], None);
        f[10] = gated(&mut self.sched, f[2], b[5], None);
        f[11] = gate(&mut self.sched, b[6], Some(b[7]), scale);
        f[12] = gate(&mut self.sched, b[8], Some(b[9]), scale);
        f[13] = (self.sched.draw_f() as f64 * (b[10] as f64 / 3.0) * scale as f64) as f32;
        for i in 0..3 {
            f[15 + i] = (cfg_byte(&self.cfg, i) / 255.0) as f32;
            f[18 + i] = (cfg_byte(&self.cfg, 3 + i) / 255.0) as f32;
        }
        f[21] = (cfg_byte(&self.cfg, 6) / 100.0) as f32;
        self.slots[slot] = f;
        scale
    }
}

fn cfg_byte(c: &Config, i: usize) -> f64 {
    c.colors[i] as f64
}

/// `rand-int % 100` gate then a float draw (`FUN_100bc970` @0x100bcc8f..).
fn gate(s: &mut Schedule, prob: u8, amount: Option<u8>, scale: f32) -> f32 {
    let rem = s.draw_i() as i32 % 100;
    if (prob as i32) < rem {
        return 0.0;
    }
    let d = s.draw_f() as f64;
    match amount {
        Some(a) => (d * (a as f64 / 3.0) * scale as f64) as f32,
        None => (d * scale as f64) as f32,
    }
}

// -- derived values -----------------------------------------------------------------------------------------------------

/// Sound levels the sky manager sets every frame (`FUN_100be767`, `FUN_100bfa11`). All 0..=1; a level of 0 stops the sound.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Levels {
    /// `SM_Sandy_Env_Rain`: `f[0]² · clamp(f[8] / 3.5, 0, 1)` (handle `+0x6c`, `FUN_100be767` @0x100be7e8..).
    pub rain: f32,
    /// `SM_Sandy_Env_Wind`: `base · max(1 − f[2], f[4])`.
    pub wind: f32,
    /// `SM_Sandy_Env_SandWind`: `base · f[3]`.
    pub sand_wind: f32,
    /// `SM_Sandy_Env_FalloutRWind`: `base · f[5]`.
    pub fallout_red_wind: f32,
    /// `SM_Sandy_Env_FalloutGWind`: `base · f[6]`.
    pub fallout_green_wind: f32,
    /// `SM_Sandy_Env_Quake`: quake time left / 2 s.
    pub quake: f32,
}

/// A snapshot of everything the client derives from the weather struct.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub fields: Fields,
    pub levels: Levels,
    /// `GAME.HighAltitudeWindX/Z` increment of the last frame.
    pub high_altitude_wind: [f32; 2],
    /// The same wind per second (`0.004 · 0.5 · (C + D)`).
    pub high_altitude_wind_rate: [f32; 2],
}

impl State {
    /// Clear sky, no wind (before any frame).
    pub const fn clear() -> State {
        State { fields: [0.0; FIELDS], levels: Levels { rain: 0.0, wind: 0.0, sand_wind: 0.0, fallout_red_wind: 0.0, fallout_green_wind: 0.0, quake: 0.0 }, high_altitude_wind: [0.0; 2], high_altitude_wind_rate: [0.0; 2] }
    }

    /// `GAME.ThickCloudsIntensity` (`FUN_100ad5db`).
    pub fn thick_clouds_intensity(&self) -> f32 {
        self.fields[2]
    }

    /// `GAME.CloudTranspariency = 1 − f[2] / 2.33333` (`_DAT_101684d0`).
    pub fn cloud_transparency(&self) -> f32 {
        (1.0 - self.fields[2] as f64 / 2.333329916000366) as f32
    }

    /// `GAME.GameStormIntensity = max f[3..=6]`.
    pub fn storm_intensity(&self) -> f32 {
        self.fields[3..7].iter().copied().fold(self.fields[3], f32::max)
    }

    /// `GAME.GamePrecipitationIntensity = f[0]`.
    pub fn precipitation_intensity(&self) -> f32 {
        self.fields[0]
    }

    /// `[s0..s6]` for `Audio::set_weather` (rain, fog, cloud, sand, ash, fallout R, fallout G).
    pub fn music_state(&self) -> [f32; 7] {
        std::array::from_fn(|i| self.fields[i])
    }

    /// Lightning strikes are scheduled only while `f[9] > 0` (`FUN_100be767` toggles `+0x68` of the rain object) and the cloud
    /// level reaches 0.4 (`FUN_100b9850`: `obj[0x18] = 0.4` is the minimum `f[2]`, `_DAT_101663d4`).
    pub fn lightning_enabled(&self) -> bool {
        self.fields[9] > 0.0 && self.fields[2] >= 0.4
    }

    /// Wind speed, m/s (`f[7]`).
    pub fn wind_speed(&self) -> f32 {
        self.fields[7]
    }
}

/// The high altitude wind (`FUN_100bfa11` + `FUN_100bf729`): a base vector `(speed, 0, 0)` plus two decaying random walks
/// `A` and `B`; `C = base + A` and `D` chases `B` (`D += (B − D) / 1000` per frame).
#[derive(Clone, Debug, Default)]
pub struct Wind {
    base: [f32; 3],
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
    strength: f32,
}

impl Wind {
    /// `FUN_100bfa11`: speed clamp, direction keeps its heading (`+X` the first time), strength `1 − (1 − v/33)³`.
    fn set_speed(&mut self, speed: f32) {
        let v = speed.clamp(0.0, 33.0).max(0.001);
        self.base = [v, 0.0, 0.0];
        let k = 1.0 - v / 33.0;
        self.strength = 1.0 - k * k * k;
    }

    /// `FUN_100bf729(0.9, 0.5)`: one call per frame.
    fn walk(&mut self, rng: &mut CrtRand) {
        const THR: f64 = 0.9 * 100.0;
        const AMP: f32 = 0.5;
        let s = (0.5 + self.strength as f64 * 0.5) as f32;
        let r = |rng: &mut CrtRand, div: f64| (((rng.rand() % 100 - 50) as f64 / div) / 0.016_000_001) as f32;
        if ((rng.rand() % 100) as f64) < THR {
            let v = [r(rng, 450.0), 0.0, r(rng, 450.0)];
            for (b, v) in self.b.iter_mut().zip(v) {
                *b += v * AMP * s;
            }
        }
        if ((rng.rand() % 100) as f64) < THR {
            let v = [r(rng, 450.0), r(rng, 800.0), r(rng, 450.0)];
            for (a, v) in self.a.iter_mut().zip(v) {
                *a += v * AMP * s;
            }
        }
        for k in 0..3 {
            self.c[k] = self.base[k] + self.a[k];
            self.a[k] /= 1.02;
            self.b[k] /= 1.02;
        }
        for k in 0..3 {
            self.d[k] += (self.b[k] - self.d[k]) / 1000.0;
        }
    }

    /// Current `HighAltitudeWind` per second (`0.004 · 0.5 · (C + D)`, x and z).
    pub fn rate(&self) -> [f32; 2] {
        [0.004 * 0.5 * (self.c[0] + self.d[0]), 0.004 * 0.5 * (self.c[2] + self.d[2])]
    }
}

/// Quake timer (`FUN_100be767` @0x100bed90, `FUN_100bd3ab`); `+0x58` is 2.0 s (`FUN_100bdb64`).
#[derive(Clone, Debug, Default)]
struct Quake {
    left: f32,
    acc: f32,
}

const QUAKE_DURATION: f32 = 2.0;

impl Quake {
    fn step(&mut self, rate: f32, dt: f32, rng: &mut CrtRand) -> f32 {
        if rate > 0.0 {
            let draw = (rng.rand() % 10000) as f64 / 100.0;
            self.acc = (draw * dt as f64 / 10.0 * rate as f64 + self.acc as f64) as f32;
            if self.acc >= 100.0 {
                self.acc -= 100.0;
                let n = ((rate as f64 / 33.33332824707031) as i32).max(1);
                let k = rng.rand() % n + 1;
                let t = QUAKE_DURATION / (4 - k) as f32;
                if self.left < t {
                    self.left = t;
                }
            }
        }
        if self.left > 0.0 {
            let level = self.left / QUAKE_DURATION;
            self.left = (self.left - dt).max(0.0);
            level
        } else {
            0.0
        }
    }
}

/// The client's sky-manager weather for one playfield.
pub struct Weather {
    area: Area,
    wind: Wind,
    quake: Quake,
    rng: CrtRand,
    haw: [f32; 2],
    quake_level: f32,
}

impl Weather {
    /// `env`: `playfield::environment::Env::raw`.
    pub fn new(env: &[u8; 26], day: u32) -> Weather {
        Weather { area: Area::new(Config::from_env(env), day), wind: Wind::default(), quake: Quake::default(), rng: CrtRand::default(), haw: [0.0; 2], quake_level: 0.0 }
    }

    /// One client frame (`FUN_100be767`): `day` = game day, `day_time` = `GameDayTime` (0..6480 s), `dt` = frame time.
    pub fn update(&mut self, day: u32, day_time: f64, dt: f32) {
        self.area.update(day, day_time, dt);
        let f = *self.area.fields();
        let r = self.wind.rate();
        // `FUN_100ad568` argument: 0.004 · C+D · dt · 0.5 (the vector of the previous frame)
        self.haw = [r[0] * dt, r[1] * dt];
        self.wind.set_speed(f[7]);
        self.wind.walk(&mut self.rng);
        self.quake_level = self.quake.step(f[12], dt, &mut self.rng);
    }

    /// Weather as the client has it `warmup` seconds (60 Hz frames) after entering the playfield at `day_time` and then
    /// frozen: the schedule is a pure function of `(env, day, day_time)`, only the wind needs history.
    pub fn sample(env: &[u8; 26], day: u32, day_time: f64, warmup: f32) -> Weather {
        let mut w = Weather::new(env, day);
        let dt = 1.0 / 60.0;
        for _ in 0..((warmup / dt) as usize).max(1) {
            w.update(day, day_time, dt);
        }
        w
    }

    pub fn state(&self) -> State {
        let f = *self.area.fields();
        let sp = f[7].clamp(0.0, 33.0).max(0.001);
        // FUN_100bfa11: (0.5 + 0.5 · min(|wind|/33, 1)) · speed/33 with |wind| = speed [INFERENCE: the hidden argument of
        // `FUN_10023a81` is the base wind vector]
        let base = (0.5 + 0.5 * (sp / 33.0).min(1.0)) * (sp / 33.0);
        let levels = Levels {
            rain: f[0] * f[0] * (f[8] / 3.5).clamp(0.0, 1.0),
            wind: base * (1.0 - f[2]).max(f[4]),
            sand_wind: base * f[3],
            fallout_red_wind: base * f[5],
            fallout_green_wind: base * f[6],
            quake: self.quake_level,
        };
        State { fields: f, levels, high_altitude_wind: self.haw, high_altitude_wind_rate: self.wind.rate() }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// An env record whose weights give every band (all weights 5 ⇒ 110 each) and wind/rain parameters.
    fn env() -> [u8; 26] {
        let mut e = [0u8; 26];
        e[..8].fill(5);
        e[8] = 2;
        e[9] = 10;
        e[10] = 3; // stream order: byte 0xb (wind-less rain minimum)
        e[11] = 7; // stream order: byte 0xa (maximum)
        e[12] = 100;
        e[13] = 100;
        e[14] = 100;
        e[15] = 60;
        e[16] = 100;
        e[17] = 60;
        e[18] = 30;
        e[0x13..0x16].copy_from_slice(&[100, 110, 120]);
        e[0x16..0x19].copy_from_slice(&[200, 190, 180]);
        e[0x19] = 12;
        e
    }

    #[test]
    fn r250_follows_the_recurrence() {
        let mut r = R250::new(0x1234_5678);
        let s0 = r.s;
        let i = r.i;
        assert_eq!(i, 0x78);
        assert_eq!(r.s[0x78], R250_INIT[0x78].wrapping_add(0x1234_5678));
        let v = r.next_u32();
        assert_eq!(v, s0[0x78 + 0x67] ^ s0[0x78]); // (0x78 + 0x67) < 250: no wrap
        assert_eq!(r.i, 0x79);
        // seeds above 0xf9 clamp the start index
        assert_eq!(R250::new(0xfe).i, 0xf9);
        // fraction in [0, 1)
        let mut r = R250::new(1);
        assert!((0..1000).all(|_| (0.0..1.0).contains(&r.next_f64())));
    }

    #[test]
    fn schedule_layout() {
        let cfg = Config::from_env(&env());
        let s = cfg.schedule(7);
        assert!(s.len() > 10 && s.len() < 80, "{}", s.len());
        let w = s.window;
        // slot-major, increasing inside a slot, gaps 120..1200
        let t: Vec<f32> = s.times().collect();
        for (k, win) in t.windows(2).enumerate() {
            let slot_a = (win[0] / w) as usize;
            let slot_b = (win[1] / w) as usize;
            if slot_a == slot_b {
                let gap = win[1] - win[0];
                assert!((119.9..=1200.1).contains(&gap), "{k}: {gap}");
            }
        }
        assert!(t.iter().all(|&x| (0.0..PERIOD).contains(&x)));
        // deterministic
        let again: Vec<f32> = cfg.schedule(7).times().collect();
        assert_eq!(t, again);
        // the day changes the schedule
        assert_ne!(t, cfg.schedule(8).times().collect::<Vec<_>>());
    }

    #[test]
    fn fresh_advance_lands_on_the_last_event_before_now() {
        let cfg = Config::from_env(&env());
        let mut s = cfg.schedule(3);
        let now = s.time_of(1000.0);
        assert!((now - 7480.0).abs() < 0.01, "{now}");
        s.advance(now);
        let times: Vec<f32> = s.times().collect();
        let want = (0..times.len()).filter(|&i| times[i] <= now).max_by(|&a, &b| times[a].total_cmp(&times[b])).unwrap();
        assert_eq!(s.next, want);
        assert_eq!(s.cursor, (want + 1) % times.len());
    }

    #[test]
    fn no_weights_means_clear_sky() {
        let mut e = env();
        e[..8].fill(0);
        let mut w = Weather::new(&e, 0);
        for k in 0..100 {
            w.update(0, 3240.0 + k as f64, 1.0 / 60.0);
        }
        let s = w.state();
        assert_eq!(s.fields[..15], [0.0; 15]);
        assert_eq!(s.thick_clouds_intensity(), 0.0);
        assert!((s.cloud_transparency() - 1.0).abs() < 1e-6);
        assert!(s.levels.wind < 1e-4 && Levels { wind: 0.0, ..s.levels } == Levels::default());
    }

    #[test]
    fn deterministic_and_in_range() {
        let a = Weather::sample(&env(), 4, 2000.0, 1.0);
        let b = Weather::sample(&env(), 4, 2000.0, 1.0);
        assert_eq!(a.state(), b.state());
        let mut seen = [false; 7];
        for step in 0..1296 * 8 {
            let w = Weather::sample(&env(), 4 + step / 1296, (step % 1296) as f64 * 5.0, 0.1);
            let s = w.state();
            assert!(s.fields.iter().all(|v| v.is_finite()));
            assert!(s.fields[..7].iter().all(|&v| (0.0..=1.0).contains(&v)), "{:?}", s.fields);
            assert!(s.fields[1] <= 0.65 + 1e-6);
            assert!((0.0..=33.0 * 2.75).contains(&s.fields[7]));
            assert!((0.0..=1.0).contains(&s.thick_clouds_intensity()));
            assert!(s.cloud_transparency() <= 1.0 && s.cloud_transparency() > 0.5);
            assert!(s.levels.rain >= 0.0 && s.levels.rain <= 1.0 && s.levels.wind <= 1.0);
            // cloud is the max of the other layers
            assert!(s.fields[2] >= s.fields[0] && s.fields[2] >= s.fields[1] && s.fields[2] >= s.fields[4]);
            for (seen, f) in seen.iter_mut().zip(&s.fields) {
                *seen |= *f > 0.0;
            }
            assert!((s.fields[15] - 100.0 / 255.0).abs() < 1e-6);
            assert!((s.fields[21] - 0.12).abs() < 1e-6);
        }
        assert!(seen.iter().all(|&b| b), "{seen:?}");
    }

    #[test]
    fn transitions_ramp_in() {
        // within one event the weather ramps up (f[2] cloud grows with the ramp) and never jumps back to a fresh draw
        let cfg = Config::from_env(&env());
        let s = cfg.schedule(4);
        let t0 = s.events.iter().map(|e| e.time).filter(|&t| (6600.0..12000.0).contains(&t)).fold(f32::MAX, f32::min);
        let day_t = (t0 - s.window) as f64;
        let mut prev: Option<f32> = None;
        let mut max_jump = 0f32;
        for k in 0..400 {
            let w = Weather::sample(&env(), 4, day_t + k as f64 * 0.5, 0.05);
            let c = w.state().fields[2];
            if let Some(p) = prev {
                max_jump = max_jump.max((c - p).abs());
            }
            prev = Some(c);
        }
        assert!(max_jump <= 1.0);
    }

    #[test]
    fn blend_follows_the_client_rules() {
        let mut a = [0.0f32; FIELDS];
        let mut b = [1.0f32; FIELDS];
        a[21] = 0.0;
        b[21] = 1.0;
        let mut d = a;
        blend(&mut d, &b, 0.0);
        assert_eq!(d, a);
        blend(&mut d, &b, 0.25);
        assert!((d[0] - 0.25).abs() < 1e-6);
        // colour weighted by density: dst has density 0, so the source colour wins entirely
        assert!((d[18] - 1.0).abs() < 1e-6);
        blend(&mut d, &b, 1.0);
        assert_eq!(d, b);
    }

    #[test]
    fn wind_is_bounded_and_deterministic() {
        let run = || {
            let mut w = Weather::new(&env(), 0);
            for _ in 0..3600 {
                w.update(0, 3000.0, 1.0 / 60.0);
            }
            w.state().high_altitude_wind_rate
        };
        let r = run();
        assert_eq!(r, run());
        assert!(r.iter().all(|v| v.is_finite() && v.abs() < 1.0), "{r:?}");
    }

    #[test]
    fn quake_levels_are_unit() {
        let mut q = Quake::default();
        let mut rng = CrtRand::default();
        let mut max = 0f32;
        for _ in 0..20000 {
            max = max.max(q.step(100.0, 1.0 / 60.0, &mut rng));
        }
        assert!(max > 0.0 && max <= 1.0);
    }
}
