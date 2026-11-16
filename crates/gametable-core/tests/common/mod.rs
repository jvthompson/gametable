//! Shared test fixtures, mirroring CardTable's `tests/helpers.lua`.
#![allow(dead_code)]

use gametable_core::models::*;

/// Builder for a card instance with the original test helper's defaults
/// (face down, on the table, at 0,0, z-index 0, definition "def").
pub struct TestCard(pub CardInstance);

pub fn card(instance_id: &str) -> TestCard {
    TestCard(CardInstance { x: 0.0, y: 0.0, face_up: false, ..CardInstance::new(instance_id, "def", CardZone::Table) })
}

impl TestCard {
    pub fn definition(mut self, definition_id: &str) -> Self {
        self.0.definition_id = definition_id.to_string();
        self
    }
    pub fn at(mut self, x: f64, y: f64) -> Self {
        self.0.x = x;
        self.0.y = y;
        self
    }
    pub fn z(mut self, z_index: i64) -> Self {
        self.0.z_index = z_index;
        self
    }
    pub fn face_up(mut self) -> Self {
        self.0.face_up = true;
        self
    }
    pub fn in_hand(mut self, owner_id: &str) -> Self {
        self.0.zone = CardZone::Hand;
        self.0.owner_id = Some(owner_id.to_string());
        self
    }
    pub fn in_zone(mut self, zone_id: &str) -> Self {
        self.0.zone = CardZone::Zone;
        self.0.zone_id = Some(zone_id.to_string());
        self
    }
    pub fn owner(mut self, owner_id: &str) -> Self {
        self.0.owner_id = Some(owner_id.to_string());
        self
    }
    pub fn on(mut self, stack_parent_id: &str) -> Self {
        self.0.stack_parent_id = Some(stack_parent_id.to_string());
        self
    }
    pub fn rotation(mut self, rotation_turns: f64) -> Self {
        self.0.rotation_turns = rotation_turns;
        self
    }
    pub fn unownable(mut self) -> Self {
        self.0.unownable = true;
        self
    }
    pub fn build(self) -> CardInstance {
        self.0
    }
}

/// A token widget at 0,0 with the given id.
pub fn widget(instance_id: &str, kind: BoardWidgetKind) -> BoardWidgetInstance {
    BoardWidgetInstance::new(instance_id, kind, 0.0, 0.0, 0)
}

/// A player with the original helper's defaults (client, black).
pub fn player(id: &str, name: &str) -> PlayerInfo {
    PlayerInfo::new(id, name, PlayerRole::Client, 0xFF000000)
}

pub fn host(id: &str, name: &str) -> PlayerInfo {
    PlayerInfo::new(id, name, PlayerRole::Host, 0xFF000000)
}

/// A table state for game "g1".
pub fn state(players: Vec<PlayerInfo>, cards: Vec<CardInstance>) -> TableState {
    let mut state = TableState::new("g1").with_cards(cards);
    state.players = players;
    state
}

/// A card definition whose title equals its id.
pub fn card_definition(id: &str, types: &[&str]) -> CardDefinition {
    CardDefinition { types: types.iter().map(|tag| tag.to_string()).collect(), ..CardDefinition::new(id, id) }
}

pub fn ids_of_cards<'a>(cards: impl IntoIterator<Item = &'a CardInstance>) -> Vec<String> {
    cards.into_iter().map(|card| card.instance_id.clone()).collect()
}

pub fn ids_of_definitions<'a>(cards: impl IntoIterator<Item = &'a CardDefinition>) -> Vec<String> {
    cards.into_iter().map(|card| card.id.clone()).collect()
}

pub fn ids_of_players<'a>(players: impl IntoIterator<Item = &'a PlayerInfo>) -> Vec<String> {
    players.into_iter().map(|player| player.id.clone()).collect()
}
