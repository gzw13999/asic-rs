use asic_rs_core::data::{
    board::{BoardData, ChipData},
    collector::{DataExtractor, DataField, DataLocation, get_by_pointer},
    command::MinerCommand,
    hashrate::{HashRate, HashRateUnit},
};
use measurements::{Power, Temperature, Voltage};
use serde_json::{Map, Value};

pub const SUMMARY_STATS: &str = "/STATS/0/MM ID0:Summary/STATS";
pub const SUMMARY_GHSMM: &str = "/STATS/0/MM ID0:Summary/STATS/GHSmm";
pub const HBINFO: &str = "/STATS/0/HBinfo";

pub fn summary_f64(summary: &Value, key: &str, idx: usize) -> Option<f64> {
    let value = summary.get(key)?;
    value
        .as_array()
        .and_then(|arr| arr.get(idx))
        .unwrap_or(value)
        .as_f64()
}

pub fn parse_summary_wattage(value: &Value) -> Option<Power> {
    if let Some(watts) = value.as_f64() {
        return Some(Power::from_watts(watts));
    }
    if let Some(watts) = value.get("WALLPOWER").and_then(Value::as_f64) {
        return Some(Power::from_watts(watts));
    }
    if let Some(ps) = value
        .get("ps")
        .or_else(|| value.get("PS"))
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
    {
        for idx in [4_usize, 5] {
            if let Some(watts) = ps.get(idx).and_then(Value::as_f64)
                && watts > 0.0
            {
                return Some(Power::from_watts(watts));
            }
        }
    }
    None
}

pub fn summary_stat_locations(
    stats: MinerCommand,
    litestats: Option<MinerCommand>,
) -> Vec<DataLocation> {
    let mut locations = vec![(stats, summary_extractor(SUMMARY_STATS, Some("summary")))];
    if let Some(litestats) = litestats {
        locations.push((litestats, summary_extractor(SUMMARY_STATS, Some("summary"))));
    }
    locations
}

pub fn summary_scalar_locations(
    stats: MinerCommand,
    litestats: Option<MinerCommand>,
    pointer: &'static str,
) -> Vec<DataLocation> {
    let mut locations = vec![(
        stats,
        DataExtractor {
            func: get_by_pointer,
            key: Some(pointer),
            tag: None,
        },
    )];
    if let Some(litestats) = litestats {
        locations.push((
            litestats,
            DataExtractor {
                func: get_by_pointer,
                key: Some(pointer),
                tag: None,
            },
        ));
    }
    locations
}

pub fn summary_wattage_locations(
    stats: MinerCommand,
    litestats: Option<MinerCommand>,
) -> Vec<DataLocation> {
    let wallpower = |cmd: MinerCommand| {
        (
            cmd,
            DataExtractor {
                func: get_by_pointer,
                key: Some("/STATS/0/MM ID0:Summary/STATS/WALLPOWER"),
                tag: None,
            },
        )
    };
    let ps = |cmd: MinerCommand| {
        (
            cmd,
            DataExtractor {
                func: get_by_pointer,
                key: Some("/STATS/0/MM ID0:Summary/STATS/PS"),
                tag: Some("ps"),
            },
        )
    };

    let mut locations = vec![wallpower(stats.clone()), ps(stats)];
    if let Some(litestats) = litestats {
        locations.push(wallpower(litestats.clone()));
        locations.push(ps(litestats));
    }
    locations
}

fn summary_extractor(pointer: &'static str, tag: Option<&'static str>) -> DataExtractor {
    DataExtractor {
        func: get_by_pointer,
        key: Some(pointer),
        tag,
    }
}

pub fn apply_summary_hashboards(
    hashboards: &mut [BoardData],
    summary: &Value,
    hb_info: Option<&Map<String, Value>>,
) {
    for board in hashboards.iter_mut() {
        let idx = board.position as usize;

        board.hashrate = summary_f64(summary, "MGHS", idx).map(|rate| {
            HashRate {
                value: rate,
                unit: HashRateUnit::GigaHash,
                algo: "SHA256".to_string(),
            }
            .as_unit(HashRateUnit::default())
        });

        board.board_temperature =
            summary_f64(summary, "HBITemp", idx).map(Temperature::from_celsius);

        board.inlet_chip_temperature =
            summary_f64(summary, "ITemp", idx).map(Temperature::from_celsius);

        board.outlet_chip_temperature =
            summary_f64(summary, "HBOTemp", idx).map(Temperature::from_celsius);

        board.active = board.hashrate.as_ref().map(|h| h.value > 0.0);
        if hb_info.is_none() {
            board.working_chips = match (board.active, board.expected_chips) {
                (Some(true), Some(expected_chips)) => Some(expected_chips),
                (Some(false), _) => Some(0),
                _ => None,
            };
        }

        let Some(hb_info) = hb_info else {
            continue;
        };

        let key = format!("HB{idx}");
        let Some(board_info) = hb_info.get(&key) else {
            continue;
        };

        let temps: Vec<f64> = board_info
            .get("PVT_T0")
            .and_then(Value::as_array)
            .map(|array| array.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();

        let volts: Vec<f64> = board_info
            .get("PVT_V0")
            .and_then(Value::as_array)
            .map(|array| array.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();

        let works: Vec<f64> = board_info
            .get("MW0")
            .and_then(Value::as_array)
            .map(|array| array.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();

        board.chips = temps
            .iter()
            .zip(volts.iter())
            .zip(works.iter())
            .enumerate()
            .map(|(pos, ((&temp, &volt), &work))| ChipData {
                position: pos as u16,
                temperature: Some(Temperature::from_celsius(temp)),
                voltage: Some(Voltage::from_millivolts(volt)),
                working: Some(work > 0.0),
                ..Default::default()
            })
            .collect();

        board.working_chips = Some(
            board
                .chips
                .iter()
                .filter(|chip| chip.working.unwrap_or(false))
                .count() as u16,
        );
    }
}

pub fn hashboards_use_summary_format(data: &std::collections::HashMap<DataField, Value>) -> bool {
    data.get(&DataField::Hashboards)
        .and_then(|value| value.get("summary"))
        .is_some()
}

pub fn parse_legacy_ps_wattage(value: &Value) -> Option<Power> {
    value
        .as_array()
        .and_then(|ps| ps.get(4))
        .and_then(Value::as_f64)
        .map(Power::from_watts)
}

pub fn parse_legacy_ps_tuning_target(value: &Value) -> Option<Power> {
    value
        .as_array()
        .and_then(|ps| ps.get(6))
        .and_then(Value::as_f64)
        .map(Power::from_watts)
}
