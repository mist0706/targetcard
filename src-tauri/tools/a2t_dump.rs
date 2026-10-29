//! `a2t-dump` — every decoded packet in a capture as one line, for analysis
//! outside Rust.
//!
//! ```text
//! a2t-dump <capture>            at_ms|opcode|hex
//! ```
//!
//! Streams reassembled and LZ4 bundles expanded, exactly as the live parser
//! sees them, so whatever a field looks like here it looks like that in the
//! meter. Made for questions of the shape "which number moves when this
//! happens", which are easier to ask of the whole capture at once.

use std::collections::HashMap;

use a2tools_dps_meter_lib::capture::framing::{self, FrameKind};
use a2tools_dps_meter_lib::capture::packet_accumulator::PacketAccumulator;
use a2tools_dps_meter_lib::capture::stream_processor::read_varint;
use a2tools_dps_meter_lib::share;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("a2t-dump <capture>");
            std::process::exit(2);
        }
    };

    fn expand(buf: &[u8], out: &mut Vec<Vec<u8>>, depth: usize, top: bool) {
        if depth > 4 {
            return;
        }
        let frames = if top {
            framing::walk(buf).frames
        } else {
            framing::walk_inner(buf).frames
        };
        for f in frames {
            match f.kind {
                FrameKind::Packet => out.push(f.bytes(buf).to_vec()),
                FrameKind::Bundle => {
                    if let Some(d) = framing::decompress_bundle(f.payload(buf)) {
                        expand(&d, out, depth + 1, false);
                    }
                }
            }
        }
    }

    let caps = match share::read_capture(std::path::Path::new(&path)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let mut streams: HashMap<String, PacketAccumulator> = HashMap::new();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    for cap in caps {
        let acc = streams
            .entry(cap.stream.clone())
            .or_insert_with(PacketAccumulator::new);
        acc.append(&cap.bytes);
        let buf = acc.snapshot().to_vec();
        let walk = framing::walk(&buf);
        let mut plain = Vec::new();
        for f in &walk.frames {
            match f.kind {
                FrameKind::Packet => plain.push(f.bytes(&buf).to_vec()),
                FrameKind::Bundle => {
                    if let Some(d) = framing::decompress_bundle(f.payload(&buf)) {
                        expand(&d, &mut plain, 1, false);
                    }
                }
            }
        }
        acc.discard_bytes(walk.consumed);

        for bytes in plain {
            let li = read_varint(&bytes, 0);
            if li.length <= 0 {
                continue;
            }
            let o = li.length as usize;
            if o + 1 >= bytes.len() {
                continue;
            }
            let op = format!("{:02X}{:02X}", bytes[o], bytes[o + 1]);
            let hex: String = bytes[o + 2..]
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect();
            use std::io::Write;
            let _ = writeln!(out, "{}|{}|{}", cap.captured_at_ms, op, hex);
        }
    }
}
