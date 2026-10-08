//! Appearance config (`config_types::Item::appearance`,
//! `pack_types::Types::install_appearance_types`, `defaults`, `titles`);
//! `protocol910::appearance` re-exports it.
use super::customisation::Customisation;
use std::collections::BTreeMap;
#[derive(Clone, Debug)]
pub struct Item {
    pub team: i32,
    pub defaults: Customisation,
}
/// Palette lengths are the lengths of each slot's first destination list;
/// title values come from the gender-selected enum (including that enum's
/// fallback).
pub struct Config {
    pub wear: Vec<i32>,
    pub colour_lengths: [usize; 10],
    pub texture_lengths: [usize; 10],
    pub items: BTreeMap<i32, Item>,
    pub npc_sizes: BTreeMap<i32, i32>,
    pub titles: BTreeMap<(i8, i32), String>,
    pub default_titles: [String; 2],
    pub staff_live_override: bool,
}
