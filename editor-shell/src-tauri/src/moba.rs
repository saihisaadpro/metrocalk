//! Run the MOB match kernel over the **authored scene** and project it onto the live wgpu viewport.
//!
//! This is the connection between the two halves of the product. `metrocalk-gameplay` was for a long time
//! physically disconnected from the editor — not a dependency, no bridge, no command surface — so the only
//! evidence a match ran was a green test count. This module makes a real match drive real meshes in the
//! real viewport, which is the only way to *look* at it.
//!
//! **The definitions come from the document.** An earlier version of this module hand-wrote the match:
//! two cores, a hero and a wave, spelled out in Rust. That proved the kernel ran and rendered, but it
//! proved nothing about authoring, because nothing the user did could reach it. The conversion now goes
//! through [`metrocalk_editor_shell::match_cook`], so what runs here is whatever is in the scene — and a
//! scene that cannot be cooked refuses to start, with the reasons pointing at the objects at fault.
//!
//! **It is a render projection, never a document mutation** (ADR-021/034). Per-tick actor transforms are
//! written into `SceneState` exactly as the physics sim's Play mode writes its own; the ECS/Loro document
//! is untouched, nothing enters the undo stack, and stopping restores the pre-match scene verbatim. The
//! cook takes `&Engine`, so that is enforced by the type system rather than by discipline. The kernel
//! stays the authority for gameplay truth — this module only asks it where things are and draws that.
//!
//! The kernel works in integer millimetres; the viewport works in metres. `MM_PER_UNIT` is the single
//! conversion, applied in one place, so no other code has to know the kernel's units.

use metrocalk_core::Engine;
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::match_cook::{
    cook_match, CookDiagnostic, CookSeverity, CookedMatch, MATCH_COOK_SCHEMA_VERSION,
};
use metrocalk_gameplay::{
    ActorId, ActorKind, CommandKind, ControlKind, MatchPhase, ModifierOp, OneLaneMatch,
    PlayerCommand, PlayerId, StatKind, Vec2Mm,
};

use crate::render::{Instance, SceneState, IDENTITY_QUAT};

/// Millimetres per viewport unit. The kernel is authoritative in millimetres and never learns about metres.
const MM_PER_UNIT: f32 = 1_000.0;

/// The local player. The cook marks exactly one authored hero as owned; it answers to this identity.
const PLAYER: PlayerId = PlayerId(1);

/// One actor as the editor surface sees it. Plain data so the React layer never touches kernel types.
#[derive(Clone, serde::Serialize)]
pub struct MobaActor {
    pub id: u64,
    pub team: u8,
    pub kind: String,
    pub x_mm: i32,
    pub y_mm: i32,
    pub health: u32,
    pub max_health: u32,
    pub alive: bool,
    pub owned: bool,
    pub controls: Vec<String>,
    pub speed: u32,
    /// The authored entity this actor was cooked from, or `None` for a unit the match spawned at runtime.
    /// This is the link that lets a viewer trace what they are watching back to what someone authored.
    pub source: Option<String>,
}

/// The observable state of a running match. Everything here is read back from the kernel.
#[derive(Clone, serde::Serialize)]
pub struct MobaStatus {
    pub running: bool,
    pub tick: u64,
    pub phase: String,
    pub world_digest: String,
    pub lane_digest: String,
    /// The digest of the cooked artifact this match was built from. Two sessions sharing this digest were
    /// built from the same authored definitions.
    pub cook_digest: String,
    pub cook_schema_version: u32,
    pub actor_count: usize,
    pub live_actors: usize,
    pub actors: Vec<MobaActor>,
    pub events: Vec<String>,
    pub last_rejection: Option<String>,
}

impl MobaStatus {
    fn idle() -> Self {
        Self {
            running: false,
            tick: 0,
            phase: "Idle".to_owned(),
            world_digest: String::new(),
            lane_digest: String::new(),
            cook_digest: String::new(),
            cook_schema_version: MATCH_COOK_SCHEMA_VERSION,
            actor_count: 0,
            live_actors: 0,
            actors: Vec::new(),
            events: Vec::new(),
            last_rejection: None,
        }
    }
}

/// Why a match could not start. Carries the cook's diagnostics so the shell can list the objects at fault
/// instead of showing one flattened string.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobaStartError {
    /// A one-line summary for a toast.
    pub message: String,
    /// Every diagnostic, errors first, each anchored to the authored entity and field.
    pub diagnostics: Vec<CookDiagnostic>,
}

/// The result of validating the scene without starting anything — what the editor shows continuously.
///
/// Deliberately snake_case on the wire, like `MobaStatus` beside it. A camelCase rename here read fine in
/// Rust and silently delivered `undefined` to every consumer — caught only by running the packaged app.
#[derive(Clone, serde::Serialize)]
pub struct MobaValidation {
    /// `true` when the scene would start.
    pub ok: bool,
    /// `true` when the scene has match settings at all — the difference between "broken" and "not a match".
    pub is_match_scene: bool,
    pub diagnostics: Vec<CookDiagnostic>,
    /// Present when the scene cooks: the digest, so the UI can show whether a recook changed anything.
    pub cook_digest: Option<String>,
    pub actor_count: usize,
    pub wave_count: usize,
    /// Lane length in metres, for the summary line.
    pub lane_length_m: f64,
}

/// A cooked scene that is ready to run, plus anything the cook wanted to say about it.
///
/// This exists because the authored document lives on the engine thread and the viewport session lives on
/// the UI thread. The cook is a read of the document, so it runs where the document is; the result crosses
/// the channel as plain data and the session is built from it. Nothing about the engine is borrowed here.
pub struct MatchPlan {
    pub cooked: CookedMatch,
    /// Non-blocking advice, already flattened to prose for the session's event log.
    pub warnings: Vec<String>,
}

/// Cook the authored scene into a runnable plan. Runs on the engine thread; the result is plain data.
///
/// # Errors
/// Returns every diagnostic, so the shell can list the objects at fault rather than one flattened string.
pub fn plan(engine: &Engine<FlecsWorld>) -> Result<MatchPlan, MobaStartError> {
    let outcome = cook_match(engine);
    let warnings = outcome
        .diagnostics
        .iter()
        .filter(|d| d.severity == CookSeverity::Warning)
        .map(|d| d.message.clone())
        .collect();
    match outcome.cooked {
        Some(cooked) => Ok(MatchPlan { cooked, warnings }),
        None => {
            let errors = outcome
                .diagnostics
                .iter()
                .filter(|d| d.severity == CookSeverity::Error)
                .count();
            Err(MobaStartError {
                message: match errors {
                    1 => "This scene cannot run a match yet — one problem needs fixing.".to_owned(),
                    n => format!("This scene cannot run a match yet — {n} problems need fixing."),
                },
                diagnostics: outcome.diagnostics,
            })
        }
    }
}

/// Validate the authored scene and report what a match would look like. Read-only; safe to call on every
/// document change, which is what makes the authoring feedback continuous rather than start-time-only.
pub fn validate(engine: &Engine<FlecsWorld>) -> MobaValidation {
    let is_match_scene = metrocalk_editor_shell::match_cook::scene_has_match(engine);
    let outcome = cook_match(engine);
    let (cook_digest, actor_count, wave_count, lane_length_m) = match &outcome.cooked {
        Some(cooked) => (
            Some(cooked.digest.clone()),
            cooked.actors.len(),
            cooked.waves.len(),
            lane_length_metres(cooked),
        ),
        None => (None, 0, 0, 0.0),
    };
    MobaValidation {
        ok: outcome.ok(),
        is_match_scene,
        diagnostics: outcome.diagnostics,
        cook_digest,
        actor_count,
        wave_count,
        lane_length_m,
    }
}

fn lane_length_metres(cooked: &CookedMatch) -> f64 {
    cooked
        .lane
        .centerline
        .windows(2)
        .map(|pair| {
            let dx = f64::from(pair[1].0 - pair[0].0);
            let dy = f64::from(pair[1].1 - pair[0].1);
            dx.hypot(dy)
        })
        .sum::<f64>()
        / f64::from(MM_PER_UNIT)
}

/// A live match plus the scene it replaced. Stopping restores the saved scene verbatim, so running a match
/// can never be a destructive act on the user's project.
pub struct MobaSession {
    game: OneLaneMatch,
    /// The cooked artifact this match was built from. Kept so status can name the digest the run belongs
    /// to and trace each actor back to the entity it came from.
    cooked: CookedMatch,
    /// The actor the local player commands, taken from the cook rather than assumed.
    hero: ActorId,
    /// The editor scene as it was before the match took over the viewport.
    saved_instances: Vec<Instance>,
    saved_ids: Vec<String>,
    saved_slots: Vec<i32>,
    unit_slot: i32,
    structure_slot: i32,
    last_events: Vec<String>,
    last_rejection: Option<String>,
}

impl MobaSession {
    /// Cook the authored scene and take over the viewport.
    ///
    /// `unit_slot`/`structure_slot` are real imported mesh slots — the same catalog the editor draws
    /// everything else from, not placeholder geometry.
    ///
    /// # Errors
    /// Returns the kernel's refusal if the artifact does not build. The viewport is left untouched in
    /// that case: a failed start is not allowed to disturb what the author was looking at.
    pub fn start(
        scene: &mut SceneState,
        plan: MatchPlan,
        unit_slot: i32,
        structure_slot: i32,
    ) -> Result<Self, MobaStartError> {
        let MatchPlan {
            cooked,
            warnings: outcome_warnings,
        } = plan;
        let game = cooked.build().map_err(|error| MobaStartError {
            // The cook validates everything the kernel validates, so reaching here means the two have
            // drifted. Say so plainly rather than blaming the author for a mismatch they cannot see.
            message: format!(
                "The match was built from the scene but the runtime refused it. This is a bug in the \
                 cook step, not in your scene: {error}"
            ),
            diagnostics: Vec::new(),
        })?;
        let hero = cooked
            .actors
            .iter()
            .find(|actor| actor.owned)
            .map(|actor| ActorId(actor.id))
            // The cook refuses a scene without exactly one owned hero, so this is unreachable; defaulting
            // rather than unwrapping keeps a future cook change from panicking the shell.
            .unwrap_or(ActorId(0));

        let mut session = Self {
            game,
            cooked,
            hero,
            saved_instances: scene.instances.clone(),
            saved_ids: scene.ids.clone(),
            saved_slots: scene.mesh_slots.clone(),
            unit_slot,
            structure_slot,
            last_events: vec!["MatchStarted".to_owned()],
            last_rejection: None,
        };
        session.last_events.extend(
            outcome_warnings
                .iter()
                .map(|w| format!("cook warning: {w}")),
        );
        session.project(scene);
        Ok(session)
    }

    /// The cooked artifact behind this session — the inspectable middle of the evidence chain.
    pub const fn cooked(&self) -> &CookedMatch {
        &self.cooked
    }

    /// Advance the authoritative match and redraw. Returns the kernel's own view of what happened.
    pub fn step(&mut self, scene: &mut SceneState, ticks: u32) -> MobaStatus {
        self.last_events.clear();
        for _ in 0..ticks.min(600) {
            if self.game.runtime().phase() != MatchPhase::Active {
                break;
            }
            match self.game.step() {
                Ok(frame) => {
                    for event in &frame.server.events {
                        self.last_events.push(format!("{event:?}"));
                    }
                }
                Err(error) => {
                    self.last_events.push(format!("step refused: {error}"));
                    break;
                }
            }
        }
        self.project(scene);
        self.status()
    }

    /// Issue the player's hero a lane-anchored move order, exactly as a game client would.
    pub fn order_move(&mut self, scene: &mut SceneState, x_mm: i32, y_mm: i32) -> MobaStatus {
        let sequence = u32::try_from(self.game.runtime().tick() + 1).unwrap_or(1);
        let command = PlayerCommand {
            player: PLAYER,
            sequence,
            execute_at: self.game.runtime().tick() + 1,
            actor: self.hero,
            kind: CommandKind::MoveTo {
                destination: Vec2Mm::new(x_mm, y_mm),
            },
        };
        self.last_rejection = match self.game.submit(command) {
            Ok(_) => None,
            Err(rejection) => Some(format!("{rejection:?}")),
        };
        self.project(scene);
        self.status()
    }

    /// Apply crowd control to the hero — the visible proof GP-02 is running in the live viewport.
    pub fn stun_hero(&mut self, scene: &mut SceneState, ticks: u32) -> MobaStatus {
        self.last_rejection = self
            .game
            .apply_status_effect(
                self.hero,
                ticks.max(1),
                &[ControlKind::Stun],
                &[(StatKind::MoveSpeed, ModifierOp::PercentAdd { bps: -5_000 })],
            )
            .err()
            .map(|error| format!("{error:?}"));
        self.project(scene);
        self.status()
    }

    /// Restore the editor scene exactly as it was before the match started.
    pub fn stop(self, scene: &mut SceneState) {
        scene.instances = self.saved_instances;
        scene.ids = self.saved_ids;
        scene.mesh_slots = self.saved_slots;
        scene.revision = scene.revision.wrapping_add(1);
    }

    /// Draw the kernel's actors with real meshes. Dead actors are dropped rather than left floating.
    fn project(&self, scene: &mut SceneState) {
        let mut instances = Vec::new();
        let mut ids = Vec::new();
        let mut slots = Vec::new();
        for actor in self.game.runtime().actors() {
            if !actor.alive {
                continue;
            }
            let structure = matches!(actor.kind, ActorKind::Structure);
            let colour = match (actor.team.get(), actor.owner.is_some()) {
                (0, true) => [0.35, 0.75, 1.0],
                (0, false) => [0.20, 0.45, 0.85],
                (_, true) => [1.0, 0.55, 0.45],
                _ => [0.85, 0.28, 0.28],
            };
            // A stunned actor is tinted so crowd control is legible in the viewport itself, not only in a
            // side panel. This is presentation of authoritative state, never a gameplay decision.
            // Colour is not the only channel: the actor is also raised and enlarged, and the status panel
            // lists the control by name, so the state survives a colour-blind or greyscale reading.
            let stunned = actor.controls.contains(ControlKind::Stun);
            let colour = if stunned { [1.0, 0.85, 0.30] } else { colour };
            let base_scale = if structure { 1.4 } else { 0.7 };
            instances.push(Instance {
                center: [
                    actor.position.x as f32 / MM_PER_UNIT,
                    if structure {
                        0.5
                    } else if stunned {
                        0.60
                    } else {
                        0.35
                    },
                    actor.position.y as f32 / MM_PER_UNIT,
                ],
                scale: if stunned {
                    base_scale * 1.25
                } else {
                    base_scale
                },
                color: colour,
                selected: 0.0,
                rotation: IDENTITY_QUAT,
                material: [0.15, 0.55, 1.0, 0.0],
            });
            // Runtime actors are deliberately NOT given authored entity keys. They are a projection, and a
            // document key here would let selection, the outliner or an edit reach a thing that does not
            // exist in the document (ADR-021/034). The authored source travels in the status payload.
            ids.push(format!("moba:{}", actor.id.get()));
            slots.push(if structure {
                self.structure_slot
            } else {
                self.unit_slot
            });
        }
        scene.instances = instances;
        scene.ids = ids;
        scene.mesh_slots = slots;
        scene.line_points.clear();
        scene.revision = scene.revision.wrapping_add(1);
    }

    pub fn status(&self) -> MobaStatus {
        let runtime = self.game.runtime();
        let actors: Vec<MobaActor> = runtime
            .actors()
            .iter()
            .map(|actor| MobaActor {
                id: actor.id.get(),
                team: actor.team.get(),
                kind: format!("{:?}", actor.kind),
                x_mm: actor.position.x,
                y_mm: actor.position.y,
                health: actor.health,
                max_health: actor.max_health,
                alive: actor.alive,
                owned: actor.owner.is_some(),
                controls: actor
                    .controls
                    .kinds()
                    .iter()
                    .map(|kind| format!("{kind:?}"))
                    .collect(),
                speed: actor.move_speed_mm_per_tick,
                source: self.cooked.source_of(actor.id.get()).map(ToOwned::to_owned),
            })
            .collect();
        MobaStatus {
            running: runtime.phase() == MatchPhase::Active,
            tick: runtime.tick(),
            phase: format!("{:?}", runtime.phase()),
            world_digest: format!("{:016x}", runtime.world_digest().0),
            lane_digest: format!("{:016x}", self.game.lane_digest().0),
            cook_digest: self.cooked.digest.clone(),
            cook_schema_version: self.cooked.schema_version,
            actor_count: actors.len(),
            live_actors: runtime.live_actor_count(),
            actors,
            events: self.last_events.clone(),
            last_rejection: self.last_rejection.clone(),
        }
    }
}

pub fn idle_status() -> MobaStatus {
    MobaStatus::idle()
}
