//! NumPy-compatible random numbers: `SeedSequence` + `PCG64` (XSL-RR) as used by
//! `numpy.random.default_rng(seed_or_list)`.
//!
//! Only the calls the project needs are implemented: `random`, `uniform`, `permutation`,
//! and `integers(0, 2, dtype=int8)`.

const INIT_A: u32 = 0x43b0_d7e5;
const MULT_A: u32 = 0x931e_8875;
const INIT_B: u32 = 0x8b51_f9dd;
const MULT_B: u32 = 0x58f3_8ded;
const MIX_MULT_L: u32 = 0xca01_f9dd;
const MIX_MULT_R: u32 = 0x4973_f715;
const XSHIFT: u32 = 16;
const POOL: usize = 4;

const PCG_MULT: u128 = 0x2360_ED05_1FC6_5DA4_4385_DF64_9FCC_F645;

/// Split integers into little-endian 32-bit words, like `SeedSequence` does.
fn entropy_words(parts: &[u64]) -> Vec<u32> {
    let mut out = Vec::new();
    for &p in parts {
        if p == 0 {
            out.push(0);
            continue;
        }
        let mut n = p;
        while n > 0 {
            out.push((n & 0xFFFF_FFFF) as u32);
            n >>= 32;
        }
    }
    out
}

fn mix(x: u32, y: u32) -> u32 {
    let r = MIX_MULT_L.wrapping_mul(x).wrapping_sub(MIX_MULT_R.wrapping_mul(y));
    r ^ (r >> XSHIFT)
}

fn pool_from_entropy(entropy: &[u32]) -> [u32; POOL] {
    let mut hc = INIT_A;
    let mut hashmix = |v: u32| {
        let mut v = v ^ hc;
        hc = hc.wrapping_mul(MULT_A);
        v = v.wrapping_mul(hc);
        v ^ (v >> XSHIFT)
    };
    let mut pool = [0u32; POOL];
    for (i, slot) in pool.iter_mut().enumerate() {
        *slot = hashmix(entropy.get(i).copied().unwrap_or(0));
    }
    for i_src in 0..POOL {
        for i_dst in 0..POOL {
            if i_src != i_dst {
                let h = hashmix(pool[i_src]);
                pool[i_dst] = mix(pool[i_dst], h);
            }
        }
    }
    for &e in entropy.iter().skip(POOL) {
        for i_dst in 0..POOL {
            let h = hashmix(e);
            pool[i_dst] = mix(pool[i_dst], h);
        }
    }
    pool
}

fn generate_state_u64(pool: &[u32; POOL], n: usize) -> Vec<u64> {
    let mut hc = INIT_B;
    let words: Vec<u32> = (0..2 * n)
        .map(|i| {
            let mut d = pool[i % POOL] ^ hc;
            hc = hc.wrapping_mul(MULT_B);
            d = d.wrapping_mul(hc);
            d ^ (d >> XSHIFT)
        })
        .collect();
    words.chunks(2).map(|c| c[0] as u64 | ((c[1] as u64) << 32)).collect()
}

pub struct Pcg64 {
    state: u128,
    inc: u128,
    has_u32: bool,
    buf_u32: u32,
}

impl Pcg64 {
    /// Same as `numpy.random.default_rng([parts...])`. One integer is `default_rng(int)`.
    pub fn from_seed(parts: &[u64]) -> Self {
        let pool = pool_from_entropy(&entropy_words(parts));
        let v = generate_state_u64(&pool, 4);
        let initstate = ((v[0] as u128) << 64) | v[1] as u128;
        let initseq = ((v[2] as u128) << 64) | v[3] as u128;
        let mut r = Pcg64 { state: 0, inc: (initseq << 1) | 1, has_u32: false, buf_u32: 0 };
        r.step();
        r.state = r.state.wrapping_add(initstate);
        r.step();
        r
    }

    fn step(&mut self) {
        self.state = self.state.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
    }

    pub fn next_u64(&mut self) -> u64 {
        self.step();
        let hi = (self.state >> 64) as u64;
        let lo = self.state as u64;
        (hi ^ lo).rotate_right((self.state >> 122) as u32)
    }

    pub fn next_u32(&mut self) -> u32 {
        if self.has_u32 {
            self.has_u32 = false;
            return self.buf_u32;
        }
        let n = self.next_u64();
        self.has_u32 = true;
        self.buf_u32 = (n >> 32) as u32;
        n as u32
    }

    /// `rng.random()`: a double in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `rng.uniform(lo, hi, size)`.
    pub fn uniform(&mut self, lo: f64, hi: f64, n: usize) -> Vec<f64> {
        (0..n).map(|_| lo + (hi - lo) * self.next_f64()).collect()
    }

    /// Value in [0, max] (NumPy `random_interval`).
    fn interval(&mut self, max: u64) -> u64 {
        if max == 0 {
            return 0;
        }
        let mut mask = max;
        for s in [1, 2, 4, 8, 16, 32] {
            mask |= mask >> s;
        }
        if max <= 0xFFFF_FFFF {
            loop {
                let v = self.next_u32() as u64 & mask;
                if v <= max {
                    return v;
                }
            }
        }
        loop {
            let v = self.next_u64() & mask;
            if v <= max {
                return v;
            }
        }
    }

    /// Value in [0, max], public for samplers outside this module.
    pub fn interval_pub(&mut self, max: u64) -> u64 {
        self.interval(max)
    }

    /// `rng.permutation(n)`.
    pub fn permutation(&mut self, n: usize) -> Vec<u64> {
        let mut a: Vec<u64> = (0..n as u64).collect();
        for i in (1..n).rev() {
            let j = self.interval(i as u64) as usize;
            a.swap(i, j);
        }
        a
    }

    /// `rng.integers(0, 2, size=n, dtype=np.int8) * 2 - 1`, as +1 / -1.
    pub fn signs_pm1(&mut self, n: usize) -> Vec<f32> {
        let mut buf = 0u32;
        let mut cnt = 0;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            if cnt == 0 {
                buf = self.next_u32();
                cnt = 3;
            } else {
                buf >>= 8;
                cnt -= 1;
            }
            let bit = ((buf & 0xFF) * 2) >> 8; // Lemire, range 2: the top bit of the byte
            out.push(if bit == 1 { 1.0 } else { -1.0 });
        }
        out
    }
}
