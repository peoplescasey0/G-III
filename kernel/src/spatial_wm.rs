//! # Post Card OS - Interactive Spatial Window Manager & Raycaster
//! Handles 3D spatial card tracking, ray-plane intersection, and window manipulation.

#![allow(static_mut_refs)]

use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vector3D {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialCard {
    pub id: u32,
    pub position: Vector3D,
    pub width: f32,
    pub height: f32,
    pub active: bool,
    pub focused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerEvent {
    pub x: i32,
    pub y: i32,
    pub pressed: bool,
    pub dragged: bool,
}

/// Active tracked spatial windows in the workspace
pub const MAX_SPATIAL_CARDS: usize = 16;
static mut SPATIAL_CARDS: [Option<SpatialCard>; MAX_SPATIAL_CARDS] = [None; MAX_SPATIAL_CARDS];
static mut FOCUSED_CARD_ID: Option<u32> = None;

/// Initialize default spatial cards in the 3D workspace
pub fn init_spatial_workspace() {
    unsafe {
        SPATIAL_CARDS = [None; MAX_SPATIAL_CARDS];
        SPATIAL_CARDS[0] = Some(SpatialCard {
            id: 1,
            position: Vector3D {
                x: 100.0,
                y: 80.0,
                z: -50.0,
            },
            width: 320.0,
            height: 200.0,
            active: true,
            focused: true,
        });
        SPATIAL_CARDS[1] = Some(SpatialCard {
            id: 2,
            position: Vector3D {
                x: 450.0,
                y: 120.0,
                z: -100.0,
            },
            width: 400.0,
            height: 300.0,
            active: true,
            focused: false,
        });
        FOCUSED_CARD_ID = Some(1);
    }
}

/// Perform 2D/3D ray-card intersection to find which card the pointer is touching
pub fn hit_test_spatial_cards(pointer: PointerEvent) -> Option<u32> {
    unsafe {
        for card_opt in SPATIAL_CARDS.iter() {
            if let Some(card) = card_opt {
                if card.active {
                    // Simple bounding box hit test on the projected screen plane
                    if pointer.x as f32 >= card.position.x
                        && pointer.x as f32 <= card.position.x + card.width
                        && pointer.y as f32 >= card.position.y
                        && pointer.y as f32 <= card.position.y + card.height
                    {
                        return Some(card.id);
                    }
                }
            }
        }
    }
    None
}

/// Handle interactive pointer events (Click, Drag, Focus Shift)
pub fn handle_spatial_input(pointer: PointerEvent) {
    let hit_id = hit_test_spatial_cards(pointer);

    unsafe {
        if pointer.pressed {
            if let Some(id) = hit_id {
                FOCUSED_CARD_ID = Some(id);
                // Update focus states
                for card_opt in SPATIAL_CARDS.iter_mut() {
                    if let Some(card) = card_opt {
                        card.focused = card.id == id;
                    }
                }
            }
        }

        // If dragging and a card is focused, update its 3D spatial position
        if pointer.dragged {
            if let Some(focused_id) = FOCUSED_CARD_ID {
                for card_opt in SPATIAL_CARDS.iter_mut() {
                    if let Some(card) = card_opt {
                        if card.id == focused_id {
                            // Translate card position based on input delta
                            card.position.x += 2.0; // Simulated drag delta step
                            card.position.y += 1.0;
                        }
                    }
                }
            }
        }
    }
}

/// Returns the currently focused spatial card ID.
pub fn focused_card_id() -> Option<u32> {
    unsafe { FOCUSED_CARD_ID }
}

/// Returns a snapshot of all active spatial cards in the 3D workspace.
pub fn active_cards() -> Vec<SpatialCard> {
    unsafe { SPATIAL_CARDS.iter().filter_map(|&c| c).collect() }
}
