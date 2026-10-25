//! Does presence actually get recorded from real traffic?
//!
//! Replays a capture and, for every player the parser managed to name, asks
//! whether any state or combat frame since naming carried their id. A player
//! the client is still rendering produces those frames several times a second,
//! so a name with no presence means someone is standing in front of the player
//! and the table would drop them anyway.
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
fn every_named_player_gets_presence_from_traffic() {
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

    let mut named = 0;
    let mut seen = 0;
    let mut without: Vec<String> = Vec::new();
    for (id, name) in storage.get_nicknames() {
        named += 1;
        if storage.entity_last_seen(id).is_some() {
            seen += 1;
        } else {
            without.push(format!("{name} (#{id})"));
        }
    }

    println!("named {named}, with presence {seen}, without {without:?}");
    assert!(named > 0, "the capture named nobody, so this proves nothing");
    assert!(
        seen * 2 >= named,
        "only {seen} of {named} named players produced presence; the hook is not firing"
    );
}
