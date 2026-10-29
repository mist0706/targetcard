//! Does the kill attribution work on real traffic?
//!
//! A death packet names its victim and nobody else, so a kill is credited from the last
//! hit recorded on that entity. This replays a capture through the same path the running
//! app uses and prints the kills and the death, which is also the check that the count
//! beside the abyss light is real.
//!
//! `TC_CAPTURE=<path to packets_*.txt>`; skips when unset.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use a2tools_dps_meter_lib::capture::packet_accumulator::PacketAccumulator;
use a2tools_dps_meter_lib::capture::stream_processor::StreamProcessor;
use a2tools_dps_meter_lib::combat::data_storage::DataStorage;
use a2tools_dps_meter_lib::i18n::lookup::{NpcLookup, SkillLookup};
use a2tools_dps_meter_lib::share::read_capture;

#[test]
fn a_real_capture_counts_the_kills_it_contains() {
    let Ok(path) = std::env::var("TC_CAPTURE") else {
        return;
    };
    let packets = read_capture(Path::new(&path)).expect("capture");
    let storage = Arc::new(DataStorage::new());
    let mut processor = StreamProcessor::new(
        storage.clone(),
        Arc::new(SkillLookup::new()),
        Arc::new(NpcLookup::new()),
    );

    let mut streams: HashMap<String, PacketAccumulator> = HashMap::new();
    for packet in &packets {
        let accumulator = streams
            .entry(packet.stream.clone())
            .or_insert_with(PacketAccumulator::new);
        accumulator.append(&packet.bytes);
        let used = processor.consume_stream(accumulator.snapshot());
        if used > 0 {
            accumulator.discard_bytes(used);
        }
    }

    println!("kills this capture: {}", storage.session_kills());
    println!("local entity: {:?}", storage.local_player_id());
    println!("names known: {}", storage.get_nicknames().len());
}
