//! Read a Debian 2.0 ar container without running dpkg, control or maintainer scripts.
use super::{check, invalid, io};
use kode_core::Result;
use kode_platform::jobs::CancellationToken;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub(super) fn unpack(package: &Path, cancel: &CancellationToken) -> Result<File> {
    let mut file = File::open(package).map_err(|e| io("open Debian package", package, e))?;
    let length = file
        .metadata()
        .map_err(|e| io("stat Debian package", package, e))?
        .len();
    let mut magic = [0; 8];
    file.read_exact(&mut magic)
        .map_err(|e| invalid(format!("Truncated Debian header: {e}")))?;
    if &magic != b"!<arch>\n" {
        return Err(invalid("Debian package must use an ar container"));
    }
    let mut position = 8u64;
    let mut data = None;
    for index in 0..16 {
        check(cancel)?;
        if position == length {
            break;
        }
        if length.saturating_sub(position) < 60 {
            return Err(invalid("Truncated Debian member header"));
        }
        let mut header = [0; 60];
        file.read_exact(&mut header)
            .map_err(|e| invalid(e.to_string()))?;
        if &header[58..] != b"`\n" {
            return Err(invalid("Invalid Debian member header"));
        }
        let name = std::str::from_utf8(&header[..16])
            .map_err(|e| invalid(e.to_string()))?
            .trim()
            .trim_end_matches('/');
        let number = std::str::from_utf8(&header[48..58])
            .map_err(|e| invalid(e.to_string()))?
            .trim();
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid("Invalid Debian member length"));
        }
        let size: u64 = number
            .parse()
            .map_err(|_| invalid("Debian member length overflow"))?;
        let end = position
            .checked_add(60)
            .and_then(|n| n.checked_add(size))
            .filter(|n| *n <= length)
            .ok_or_else(|| invalid("Debian member exceeds the package"))?;
        match (index, name) {
            (0, "debian-binary") if size == 4 => {
                let mut version = [0; 4];
                file.read_exact(&mut version)
                    .map_err(|e| invalid(e.to_string()))?;
                if &version != b"2.0\n" {
                    return Err(invalid("Unsupported Debian package version"));
                }
            }
            (1, "control.tar" | "control.tar.gz" | "control.tar.xz" | "control.tar.zst") => {}
            (2, "data.tar.zst") => {
                // Bound allocations before an untrusted header requests its window.
                let mut frame = ruzstd::decoding::FrameDecoder::new();
                frame.set_max_window_size(32 * 1024 * 1024);
                let mut decoder = ruzstd::decoding::StreamingDecoder::new_with_decoder(
                    (&mut file).take(size),
                    frame,
                )
                .map_err(|e| invalid(format!("Invalid Debian Zstandard data: {e}")))?;
                data = Some(super::unpack(&mut decoder, package, cancel)?);
                if decoder.into_inner().limit() != 0 {
                    return Err(invalid("Trailing Debian Zstandard data is unsupported"));
                }
            }
            (2, "data.tar.gz") => {
                let mut decoder = flate2::read::GzDecoder::new((&mut file).take(size));
                data = Some(super::unpack(&mut decoder, package, cancel)?);
            }
            (2, "data.tar") => {
                data = Some(super::unpack(&mut (&mut file).take(size), package, cancel)?)
            }
            (2, _) => {
                return Err(invalid(
                    "Debian data requires TAR, GZIP or Zstandard compression",
                ));
            }
            _ => return Err(invalid("Unexpected or duplicate Debian package member")),
        }
        position = end;
        if !size.is_multiple_of(2) {
            file.seek(SeekFrom::Start(position))
                .map_err(|e| invalid(e.to_string()))?;
            let mut pad = [0];
            file.read_exact(&mut pad)
                .map_err(|e| invalid(format!("Truncated Debian padding: {e}")))?;
            if pad != *b"\n" {
                return Err(invalid("Invalid Debian member padding"));
            }
            position += 1;
        }
        file.seek(SeekFrom::Start(position))
            .map_err(|e| invalid(e.to_string()))?;
    }
    if position != length {
        return Err(invalid("Debian package member limit exceeded"));
    }
    data.ok_or_else(|| invalid("Debian package has no data archive"))
}
