//! Dependency-free stored ZIP writer + SHA-256 inventory for qin-board.seplugin
//! (copied from se-plugin-pack; the service path is native/qin-board-service). Run with --manifest --target --native --ui --service --out.
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut options = BTreeMap::new();
    while let Some(key) = args.next() {
        options.insert(key, args.next().ok_or("missing option value")?);
    }
    let get = |key: &str| {
        options
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("missing {key}"))
    };
    let manifest = fs::read(get("--manifest")?)?;
    // Reject malformed JSON before packaging.
    serde_json::from_slice::<serde_json::Value>(&manifest)?;
    let target = get("--target")?;
    let native = Path::new(get("--native")?);
    let native_path = format!(
        "native/{target}/{}",
        native
            .file_name()
            .ok_or("native filename")?
            .to_str()
            .ok_or("native UTF-8")?
    );
    let mut entries = BTreeMap::from([
        ("manifest.json".to_owned(), manifest),
        (native_path.clone(), fs::read(native)?),
        (
            "native/qin-board-service".to_owned(),
            fs::read(get("--service")?)?,
        ),
    ]);
    collect(
        Path::new(get("--ui")?),
        Path::new(get("--ui")?),
        &mut entries,
    )?;
    let inventory: Vec<_> = entries
        .iter()
        .map(|(path, bytes)| json!({"path":path,"size":bytes.len(),"sha256":sha256(bytes)}))
        .collect();
    entries.insert(
        "package.json".into(),
        serde_json::to_vec_pretty(
            &json!({"schemaVersion":1,"native":{target:native_path},"ui":"ui","files":inventory}),
        )?,
    );
    let output = Path::new(get("--out")?);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    write_zip(output, entries)?;
    println!("{}", output.display());
    Ok(())
}
fn collect(
    root: &Path,
    directory: &Path,
    entries: &mut BTreeMap<String, Vec<u8>>,
) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(io::Error::other("UI symlinks are not permitted"));
        }
        if kind.is_dir() {
            collect(root, &path, entries)?;
        } else if kind.is_file() {
            let relative: PathBuf = path.strip_prefix(root).map_err(io::Error::other)?.into();
            entries.insert(
                format!("ui/{}", relative.to_string_lossy().replace('\\', "/")),
                fs::read(path)?,
            );
        }
    }
    Ok(())
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
fn u16le(out: &mut Vec<u8>, n: u16) {
    out.extend(n.to_le_bytes());
}
fn u32le(out: &mut Vec<u8>, n: u32) {
    out.extend(n.to_le_bytes());
}
fn write_zip(path: &Path, entries: BTreeMap<String, Vec<u8>>) -> io::Result<()> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let count = entries.len() as u16;
    for (name, bytes) in entries {
        let offset = out.len() as u32;
        let size = u32::try_from(bytes.len()).map_err(io::Error::other)?;
        let crc = crc32(&bytes);
        u32le(&mut out, 0x04034b50);
        u16le(&mut out, 20);
        u16le(&mut out, 0);
        u16le(&mut out, 0);
        u16le(&mut out, 0);
        u16le(&mut out, 0);
        u32le(&mut out, crc);
        u32le(&mut out, size);
        u32le(&mut out, size);
        u16le(&mut out, name.len() as u16);
        u16le(&mut out, 0);
        out.extend(name.as_bytes());
        out.extend(bytes);
        u32le(&mut central, 0x02014b50);
        u16le(&mut central, 0x0314);
        u16le(&mut central, 20);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u32le(&mut central, crc);
        u32le(&mut central, size);
        u32le(&mut central, size);
        u16le(&mut central, name.len() as u16);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u32le(&mut central, 0o100755 << 16);
        u32le(&mut central, offset);
        central.extend(name.as_bytes());
    }
    let offset = out.len() as u32;
    let size = central.len() as u32;
    out.extend(central);
    u32le(&mut out, 0x06054b50);
    u16le(&mut out, 0);
    u16le(&mut out, 0);
    u16le(&mut out, count);
    u16le(&mut out, count);
    u32le(&mut out, size);
    u32le(&mut out, offset);
    u16le(&mut out, 0);
    fs::File::create(path)?.write_all(&out)
}
fn sha256(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut data = bytes.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend((bytes.len() as u64 * 8).to_be_bytes());
    for chunk in data.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, b) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*b);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let t1 = hh
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (value, next) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *value = value.wrapping_add(next);
        }
    }
    h.iter().map(|v| format!("{v:08x}")).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha_vectors() {
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256(&[b'a'; 1000000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
    }
}
