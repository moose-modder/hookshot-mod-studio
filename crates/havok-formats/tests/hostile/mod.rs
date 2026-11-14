//! Hostile-input checks of the packfile readers, shared by the synthetic and the real-data tests: a file truncated at 200
//! places is refused at each, a file with one byte flipped at each of 2,000 places (a fixed generator, so a failure
//! repeats) makes no reader panic, and garbage is not a packfile.
#![allow(dead_code, reason = "each test file uses part of this")]
use havok_formats::packfile::Packfile;

/// A fixed linear congruential generator (Numerical Recipes), so a failure repeats.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

/// Everything the packfile readers do with a file: container, image, write. None of it may panic.
pub fn use_everything(bytes: &[u8]) {
    if let Ok(pack) = Packfile::read(bytes) {
        let _ = havok_formats::packfile::physics::census(&pack);
        for object in pack.objects() {
            let _ = pack.class_name(object);
            let _ = pack.array_header(object.offset);
            let _ = pack.pointer(object.offset);
            let _ = pack.c_string(object.offset);
        }
        if let Ok(image) = pack.image() {
            let _ = image.write();
        }
    }
}

/// Truncated at every one of 200 places a file is refused; with one byte flipped at each of 2,000 places nothing panics;
/// garbage is not a packfile.
pub fn hostile(name: &str, bytes: &[u8]) {
    assert!(Packfile::read(bytes).is_ok(), "{name}: the intact file reads");
    let len = bytes.len();
    for i in 0..200usize {
        let cut = i * len / 200;
        assert!(
            Packfile::read(&bytes[..cut]).is_err(),
            "{name}: cut at {cut} of {len} bytes reads"
        );
    }
    let mut lcg = Lcg(0x5EED_0001);
    for _ in 0..2000 {
        let mut copy = bytes.to_vec();
        let at = lcg.next() as usize % len;
        copy[at] ^= (lcg.next() % 255) as u8 + 1;
        use_everything(&copy);
    }
}
