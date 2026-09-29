//! PSX memory-card format conversion between Adrenaline's `.VMP` container
//! and the raw 128KB memory card format the sync server stores and
//! RetroArch's PSX cores use directly.
//!
//! The signature algorithm in `generate_signature` is a Rust translation of
//! `generateHash` in `vmp_resign.c` from bucanero/apollo-psp (GPL-3.0):
//! <https://github.com/bucanero/apollo-psp/blob/master/source/vmp_resign.c>
//! The from-scratch header layout in `raw_to_vmp`'s no-template path (which
//! fields get set, and that the rest of the header is left zeroed) matches
//! `main.c`'s mcr-to-vmp path in the original `vita-mcr2vmp` by @dots_tb
//! (<https://github.com/dots-tb/vita-mcr2vmp/>, GPL-3.0), which
//! `vmp_resign.c` itself credits as the source of this algorithm, with help
//! from the CBPS Discord, @AnalogMan151 and @teakhanirons. save-sync,
//! vita-mcr2vmp and apollo-psp are all GPL-3.0.

use aes::{
    cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit},
    Aes128,
};
use sha1::{Digest, Sha1};

/// Little-endian value of the 4-byte magic at the start of a VMP file.
const VMP_MAGIC: u32 = 0x564D_5000;
const VMP_SEED_OFFSET: usize = 0x0C;
const VMP_HASH_OFFSET: usize = 0x20;
const VMP_HEADER_SIZE: usize = 0x80;
/// Offset of the 4-byte (LE u32) field holding the header size / MCR payload
/// offset — always `VMP_HEADER_SIZE` (0x80). Set by the reference converter
/// on every VMP it creates from scratch; a real field, not one of the
/// remaining unexamined header bytes.
const VMP_MCR_OFFSET_FIELD: usize = 0x04;
/// A VMP file: 0x80-byte header + a standard 128KB PSX memory card.
pub const VMP_SIZE: usize = 0x20080;
/// A standard PSX memory card, no header.
pub const MC_SIZE: usize = 0x20000;

const VMP_PSX_KEY: [u8; 16] = [
    0xAB, 0x5A, 0xBC, 0x9F, 0xC1, 0xF4, 0x9D, 0xE6, 0xA0, 0x51, 0xDB, 0xAE, 0xFA, 0x51, 0x88, 0x59,
];
const VMP_IV: [u8; 16] = [
    0xB3, 0x0F, 0xFE, 0xED, 0xB7, 0xDC, 0x5E, 0xB7, 0x13, 0x3D, 0xA6, 0x0D, 0x1B, 0x6B, 0x2C, 0xDC,
];

/// The two save file names Adrenaline uses for a PSX title's memory cards,
/// found under its (otherwise ordinary) PSP save folder, paired with the
/// name the raw (headerless) form is archived/hashed under. This project
/// syncs across devices (Vita, the desktop hub, RetroArch elsewhere), so an
/// archive entry actually holding raw bytes must not be named `.VMP` — a
/// `.vmp` extension is a real format claim to anything else that reads it.
/// `.mcr` matches the extension the hub's own RetroArch save scanner
/// already recognizes for raw PSX memory cards.
const VMP_TO_RAW_NAMES: [(&str, &str); 2] =
    [("SCEVMC0.VMP", "SCEVMC0.MCR"), ("SCEVMC1.VMP", "SCEVMC1.MCR")];

/// Basename of `name_or_path` — every lookup below takes either a bare
/// filename or a path (zip entry names can carry a subfolder prefix, e.g.
/// `SLUS01041/SCEVMC0.VMP`), and only the filename component is meaningful.
fn basename(name_or_path: &str) -> &str {
    name_or_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name_or_path)
}

pub fn is_vmp_filename(name_or_path: &str) -> bool {
    let name = basename(name_or_path);
    VMP_TO_RAW_NAMES
        .iter()
        .any(|(vmp, _)| name.eq_ignore_ascii_case(vmp))
}

/// The archive/hash entry name to use in place of `name`, if `name` is one
/// of Adrenaline's VMP save files.
pub fn raw_entry_name(name_or_path: &str) -> Option<&'static str> {
    let name = basename(name_or_path);
    VMP_TO_RAW_NAMES
        .iter()
        .find(|(vmp, _)| name.eq_ignore_ascii_case(vmp))
        .map(|(_, raw)| *raw)
}

/// The live Adrenaline filename a raw-named archive entry restores to.
pub fn vmp_entry_name(raw_name_or_path: &str) -> Option<&'static str> {
    let raw_name = basename(raw_name_or_path);
    VMP_TO_RAW_NAMES
        .iter()
        .find(|(_, raw)| raw_name.eq_ignore_ascii_case(raw))
        .map(|(vmp, _)| *vmp)
}

/// Strips a VMP file down to the raw memory card it wraps. `None` if `vmp`
/// isn't actually a VMP file (wrong size or magic) — the signature itself
/// is never checked, only that there's a plausible header to remove.
pub fn vmp_to_raw(vmp: &[u8]) -> Option<Vec<u8>> {
    if vmp.len() != VMP_SIZE {
        return None;
    }
    if u32::from_le_bytes(vmp[0..4].try_into().ok()?) != VMP_MAGIC {
        return None;
    }
    Some(vmp[VMP_HEADER_SIZE..].to_vec())
}

/// Wraps a raw memory card into a freshly-signed VMP file. Reuses
/// `existing_header`'s header bytes when given a valid one — preserving
/// whatever else Sony's real header format defines beyond magic/seed/
/// signature exactly as that file already had it — otherwise builds a
/// header from scratch the same way the original `vita-mcr2vmp` `main.c`
/// does when converting a raw MCR with no prior VMP to work from: a zeroed
/// 0x80-byte header with just the magic and the header-size field set.
/// `None` if `raw` isn't exactly one memory card's worth of data, or if
/// `existing_header` was given but isn't actually a valid VMP header — that
/// case fails rather than silently falling back to a from-scratch header,
/// since a caller passing `Some` is asserting it found a real template on
/// disk.
pub fn raw_to_vmp(raw: &[u8], existing_header: Option<&[u8]>) -> Option<Vec<u8>> {
    if raw.len() != MC_SIZE {
        return None;
    }

    let mut vmp = match existing_header {
        None => {
            let mut fresh = vec![0u8; VMP_SIZE];
            fresh[0..4].copy_from_slice(&VMP_MAGIC.to_le_bytes());
            fresh[VMP_MCR_OFFSET_FIELD..VMP_MCR_OFFSET_FIELD + 4]
                .copy_from_slice(&(VMP_HEADER_SIZE as u32).to_le_bytes());
            fresh
        }
        // A header was offered but it isn't actually a valid VMP (wrong size
        // or magic) — fail rather than silently falling through to a
        // from-scratch header. A caller passing `Some` is asserting it found
        // a real template on disk; treating "found something, but it's
        // garbage" the same as "nothing found" would defeat the whole reason
        // `existing_header` exists (see doc comment above).
        Some(existing)
            if existing.len() != VMP_SIZE
                || u32::from_le_bytes(existing[0..4].try_into().ok()?) != VMP_MAGIC =>
        {
            return None;
        }
        Some(existing) => {
            // Only the header (first VMP_HEADER_SIZE bytes) is ever kept —
            // the body gets overwritten with `raw` below regardless.
            let mut vmp = vec![0u8; VMP_SIZE];
            vmp[..VMP_HEADER_SIZE].copy_from_slice(&existing[..VMP_HEADER_SIZE]);
            vmp
        }
    };
    // Seed is always zero, matching vita-mcr2vmp's from-scratch main.c (a
    // calloc'd buffer, never written before hashing). The signature scheme
    // derives its HMAC key from the seed via a fixed AES dance keyed on
    // VMP_PSX_KEY/VMP_IV, not from any property of the seed value itself —
    // any 20 bytes work as long as they're written to the header before
    // signing, so there's no need for the reference resign tool's fixed
    // watermark string here.
    let seed = [0u8; 20];
    vmp[VMP_SEED_OFFSET..VMP_SEED_OFFSET + 20].copy_from_slice(&seed);
    // Hash field is zeroed here (whether starting fresh or from an existing
    // header) — generate_signature hashes the buffer with it zeroed,
    // matching the reference implementation.
    for b in vmp[VMP_HASH_OFFSET..VMP_HASH_OFFSET + 20].iter_mut() {
        *b = 0;
    }
    vmp[VMP_HEADER_SIZE..].copy_from_slice(raw);

    let signature = generate_signature(&seed, &vmp);
    vmp[VMP_HASH_OFFSET..VMP_HASH_OFFSET + 20].copy_from_slice(&signature);

    Some(vmp)
}

/// Port of `generateHash` in `vmp_resign.c` (see module docs for source).
/// HMAC-SHA1 with a 64-byte key derived from `seed` via a fixed AES-128
/// dance. Every step (including the parts that look like they discard work,
/// like zeroing bytes 20..64 of `salt` right after computing them) is
/// intentional — the algorithm must match the reference bit-for-bit or the
/// signature simply won't validate.
fn generate_signature(seed: &[u8; 20], full_buf: &[u8]) -> [u8; 20] {
    let cipher = Aes128::new(GenericArray::from_slice(&VMP_PSX_KEY));

    let mut work_buf = [0u8; 20];
    work_buf[..16].copy_from_slice(&seed[..16]);

    let mut salt = [0u8; 64];

    let mut block = GenericArray::clone_from_slice(&work_buf[..16]);
    cipher.decrypt_block(&mut block);
    for i in 0..16 {
        salt[i] = block[i] ^ VMP_IV[i];
    }

    let mut block = GenericArray::clone_from_slice(&work_buf[..16]);
    cipher.encrypt_block(&mut block);

    let mut work_buf2 = [0xFFu8; 20];
    work_buf2[..4].copy_from_slice(&seed[16..20]);
    for i in 0..16 {
        salt[16 + i] = block[i] ^ work_buf2[i];
    }

    // Zeroing bytes 20..64 wipes part of what was just written to
    // salt[16..32) (only salt[16..20) survives) — this matches the
    // reference implementation exactly, quirk and all.
    for b in salt[20..64].iter_mut() {
        *b = 0;
    }

    // HMAC inner hash: SHA1(salt ^ ipad || message).
    for b in salt.iter_mut() {
        *b ^= 0x36;
    }
    let mut hasher = Sha1::new();
    hasher.update(salt);
    hasher.update(full_buf);
    let inner: [u8; 20] = hasher.finalize().into();

    // HMAC outer hash: re-XOR the same (already ipad'd) buffer with 0x6A —
    // 0x36 ^ 0x6A == 0x5C, HMAC's opad byte — matching the reference's
    // cumulative in-place XOR rather than deriving opad independently.
    for b in salt.iter_mut() {
        *b ^= 0x6A;
    }
    let mut hasher = Sha1::new();
    hasher.update(salt);
    hasher.update(inner);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_to_vmp_round_trips() {
        let raw = vec![0x42u8; MC_SIZE];
        let vmp = raw_to_vmp(&raw, None).expect("valid raw input");
        assert_eq!(vmp.len(), VMP_SIZE);
        let back = vmp_to_raw(&vmp).expect("valid vmp output");
        assert_eq!(back, raw);
    }

    #[test]
    fn raw_to_vmp_from_scratch_matches_reference_layout() {
        // Regression test for a real bug: this from-scratch path used to
        // leave the header-size field (offset 0x04) unset and use the
        // wrong (resign-tool) seed, silently producing a header that
        // doesn't match what the reference `vita-mcr2vmp` `main.c` writes
        // when building a VMP with no prior file to work from.
        let raw = vec![0x42u8; MC_SIZE];
        let vmp = raw_to_vmp(&raw, None).expect("valid raw input");
        assert_eq!(
            u32::from_le_bytes(vmp[4..8].try_into().unwrap()),
            VMP_HEADER_SIZE as u32,
            "header-size field at offset 0x04 must be 0x80"
        );
        assert_eq!(
            &vmp[VMP_SEED_OFFSET..VMP_SEED_OFFSET + 20],
            &[0u8; 20],
            "from-scratch seed must be all-zero, matching main.c's calloc'd buffer"
        );
    }

    #[test]
    fn raw_to_vmp_reuses_existing_header() {
        // Build a "device" VMP whose header has non-magic/seed/hash bytes
        // set, then repack new payload data into it — those extra header
        // bytes must survive untouched.
        let mut existing = raw_to_vmp(&vec![0x11u8; MC_SIZE], None).unwrap();
        existing[0x50] = 0xAB; // a header byte this port never assigns meaning to
        let raw = vec![0x42u8; MC_SIZE];
        let vmp = raw_to_vmp(&raw, Some(&existing)).expect("valid raw input with header");
        assert_eq!(vmp[0x50], 0xAB);
        assert_eq!(vmp_to_raw(&vmp).unwrap(), raw);
    }

    #[test]
    fn raw_to_vmp_rejects_invalid_existing_header() {
        // A caller passing `Some` is claiming it found a real template on
        // disk. If that buffer turns out not to be a valid VMP (wrong size,
        // or right size but bad magic — e.g. a stale/corrupt file), this
        // must fail loudly rather than quietly falling back to a
        // from-scratch header.
        let raw = vec![0x42u8; MC_SIZE];
        assert!(raw_to_vmp(&raw, Some(&vec![0u8; VMP_SIZE - 1])).is_none());
        let mut bad_magic = vec![0u8; VMP_SIZE];
        bad_magic[0..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(raw_to_vmp(&raw, Some(&bad_magic)).is_none());
    }

    #[test]
    fn raw_to_vmp_rejects_wrong_size() {
        assert!(raw_to_vmp(&vec![0u8; MC_SIZE - 1], None).is_none());
        assert!(raw_to_vmp(&vec![0u8; MC_SIZE + 1], None).is_none());
    }

    #[test]
    fn vmp_to_raw_rejects_wrong_size_or_magic() {
        assert!(vmp_to_raw(&vec![0u8; VMP_SIZE - 1]).is_none());
        let mut bad_magic = vec![0u8; VMP_SIZE];
        bad_magic[0..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(vmp_to_raw(&bad_magic).is_none());
    }

    #[test]
    fn is_vmp_filename_matches_case_insensitively() {
        assert!(is_vmp_filename("SCEVMC0.VMP"));
        assert!(is_vmp_filename("scevmc1.vmp"));
        assert!(!is_vmp_filename("SCEVMC2.VMP"));
        assert!(!is_vmp_filename("PARAM.SFO"));
    }

    #[test]
    fn entry_name_mapping_round_trips() {
        assert_eq!(raw_entry_name("SCEVMC0.VMP"), Some("SCEVMC0.MCR"));
        assert_eq!(raw_entry_name("SCEVMC1.VMP"), Some("SCEVMC1.MCR"));
        assert_eq!(raw_entry_name("PARAM.SFO"), None);
        assert_eq!(vmp_entry_name("SCEVMC0.MCR"), Some("SCEVMC0.VMP"));
        assert_eq!(vmp_entry_name("scevmc1.mcr"), Some("SCEVMC1.VMP"));
        assert_eq!(vmp_entry_name("SCEVMC0.VMP"), None);
    }
}
