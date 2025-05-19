use bevy::{prelude::*, utils::HashSet};
use bevy_rapier2d::dynamics::RigidBody;
use leafwing_input_manager::action_state::ActionState;

use serde::Deserialize;
use serde::Serialize;
use tungstenite::WebSocket;

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::sync::Mutex;

use crate::player;
use crate::{
    berries::{Berry, BerryBundle},
    gates::{GateBundle, GATE_HEIGHT, GATE_NEUTRAL_IDX},
    platforms::{PlatformBundle, PLATFORM_HEIGHT},
    player::{Action, Player, PlayerController, Queen, SpawnPlayerEvent, Team},
    ship::RidingOnShip,
    GameState, WINDOW_BOTTOM_Y, WINDOW_HEIGHT, WINDOW_RIGHT_X, WINDOW_WIDTH,
};

const TEMP_PLATFORM_COLOR: Color = Color::BLACK;
pub struct JoinPlugin;

#[derive(Resource, Default)]
pub struct JoinedGamepads(pub HashSet<Gamepad>);

#[derive(Resource, Default)]
pub struct JoinedWebSockets(pub HashSet<i32>);

#[derive(Resource, Default)]
pub struct WebSocketControllers(pub HashMap<i32, ControllerState>);

impl Plugin for JoinPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<JoinedGamepads>()
            .insert_resource(JoinedWebSockets::default())
            .insert_resource(WebSocketControllers::default())
            .add_systems(
                Update,
                (
                    (check_for_start_game, disconnect).run_if(in_state(GameState::Join)),
                    join,
                    join_from_websocket,
                ),
            )
            .add_systems(OnEnter(GameState::Join), setup_join)
            .add_systems(OnExit(GameState::Join), delete_temp_platforms);
    }
}

fn check_for_start_game(
    mut next_state: ResMut<NextState<GameState>>,
    join_gates: Query<Has<Team>, With<JoinGate>>,
) {
    if join_gates.iter().all(|x| x) {
        next_state.set(GameState::Play);
    }
}

#[derive(Component)]
pub struct TempPlatform;

#[derive(Component)]
pub struct JoinGate;

fn setup_join(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
) {
    for sign in [-1.0, 1.0] {
        commands.spawn((
            PlatformBundle::new(
                sign * (WINDOW_RIGHT_X - WINDOW_WIDTH / 40.0 - WINDOW_WIDTH / 10.0
                    + WINDOW_WIDTH / 60.0),
                WINDOW_BOTTOM_Y + 7.0 * WINDOW_HEIGHT / 9.0,
                Vec3::new(
                    (WINDOW_RIGHT_X - WINDOW_WIDTH / 20.0)
                        - (WINDOW_RIGHT_X - WINDOW_WIDTH / 5.0 + WINDOW_WIDTH / 30.0),
                    PLATFORM_HEIGHT / 4.0,
                    1.0,
                ),
                true,
                Some(TEMP_PLATFORM_COLOR),
                &asset_server,
            ),
            TempPlatform,
        ));
        commands.spawn((
            PlatformBundle::new(
                sign * (((WINDOW_WIDTH / 10.0)
                    + (WINDOW_RIGHT_X - WINDOW_WIDTH / 5.0 - WINDOW_WIDTH / 30.0))
                    / 2.0),
                WINDOW_BOTTOM_Y + 7.0 * WINDOW_HEIGHT / 9.0,
                Vec3::new(
                    (WINDOW_RIGHT_X - WINDOW_WIDTH / 5.0 - WINDOW_WIDTH / 30.0)
                        - WINDOW_WIDTH / 10.0,
                    PLATFORM_HEIGHT / 4.0,
                    1.0,
                ),
                true,
                Some(TEMP_PLATFORM_COLOR),
                &asset_server,
            ),
            TempPlatform,
        ));

        commands.spawn((
            GateBundle::new(
                (WINDOW_RIGHT_X - WINDOW_WIDTH / 3.2) * sign,
                WINDOW_BOTTOM_Y + 8.0 * WINDOW_HEIGHT / 9.0 + GATE_HEIGHT / 2.0,
                &asset_server,
                &mut atlases,
            ),
            JoinGate,
        ));
    }
}

fn delete_temp_platforms(
    mut commands: Commands,
    temp_platforms: Query<Entity, With<TempPlatform>>,
    join_gates: Query<Entity, With<JoinGate>>,
) {
    for temp_platform in temp_platforms.iter() {
        commands.entity(temp_platform).despawn();
    }
    for join_gate in join_gates.iter() {
        commands.entity(join_gate).despawn();
    }
}

fn join(
    mut joined_gamepads: ResMut<JoinedGamepads>,
    gamepads: Res<Gamepads>,
    button_inputs: Res<ButtonInput<GamepadButton>>,
    queens: Query<&Team, With<Queen>>,
    mut ev_spawn_players: EventWriter<SpawnPlayerEvent>,
) {
    for gamepad in gamepads.iter() {
        // Join the game when both bumpers (L+R) on the controller are pressed
        // We drop down the Bevy's input to get the input from each gamepad
        if button_inputs.any_just_pressed([
            GamepadButton::new(gamepad, GamepadButtonType::LeftTrigger),
            GamepadButton::new(gamepad, GamepadButtonType::RightTrigger),
        ]) {
            let team = if button_inputs
                .just_pressed(GamepadButton::new(gamepad, GamepadButtonType::LeftTrigger))
            {
                Team::Yellow
            } else {
                Team::Purple
            };
            let is_queen = !queens.iter().any(|&queen_team| queen_team == team);

            // Make sure a player cannot join twice
            if !joined_gamepads.0.contains(&gamepad) {
                ev_spawn_players.send(SpawnPlayerEvent {
                    team,
                    is_queen,
                    player_controller: PlayerController::Gamepad(gamepad),
                    delay: 0.0,
                    start_invincible: false,
                });
                // Insert the created player and its gamepad to the hashmap of joined players
                // Since uniqueness was already checked above, we can insert here unchecked
                joined_gamepads.0.insert(gamepad);
            }
        }
    }
}

#[derive(Resource)]
pub struct BevyReceiver(pub Arc<Mutex<Receiver<ControllerState>>>);

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct ControllerState {
    player: i32,
    is_purple: bool,
    is_leaving: bool,
    x_movement: f32,
    jump: bool,
}

fn join_from_websocket(
    mut commands: Commands,
    mut joined_websockets: ResMut<JoinedWebSockets>,
    mut web_socket_controllers: ResMut<WebSocketControllers>,
    receiver: Res<BevyReceiver>,
    action_query: Query<(
        Entity,
        &ActionState<Action>,
        &Player,
        Has<Berry>,
        &Transform,
        Option<&RidingOnShip>,
        &Team,
        Has<Queen>,
    )>,
    queens: Query<&Team, With<Queen>>,
    mut ev_spawn_players: EventWriter<SpawnPlayerEvent>,
    asset_server: Res<AssetServer>,
    mut join_gates: Query<(Entity, &Team, &mut TextureAtlas), With<JoinGate>>,
) {
    match receiver.0.lock() {
        Ok(receiver) => {
            while let Ok(controller_update) = receiver.try_recv() {
                let player_id = controller_update.player.clone();
                if joined_websockets.0.contains(&player_id) {
                    if controller_update.is_leaving {
                        for (
                            player_entity,
                            _,
                            player,
                            killed_has_berry,
                            killed_player_transform,
                            maybe_riding_on_ship,
                            team,
                            is_queen,
                        ) in action_query.iter()
                        {
                            match player.player_controller {
                                WebSocket => {
                                    remove_player(
                                        &mut commands,
                                        player_entity,
                                        killed_has_berry,
                                        killed_player_transform,
                                        &asset_server,
                                        maybe_riding_on_ship,
                                    );
                                    if is_queen {
                                        for (join_gate, join_gate_team, mut gate_sprite) in
                                            join_gates.iter_mut()
                                        {
                                            if join_gate_team == team {
                                                commands.entity(join_gate).remove::<Team>();
                                                gate_sprite.index = GATE_NEUTRAL_IDX;
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        joined_websockets.0.remove(&player_id);
                    } else {
                        web_socket_controllers
                            .0
                            .insert(player_id, controller_update);
                    }
                } else {
                    println!("player joining");
                    let team = if controller_update.is_purple {
                        Team::Purple
                    } else {
                        Team::Yellow
                    };
                    let is_queen = !queens.iter().any(|&queen_team| queen_team == team);
                    ev_spawn_players.send(SpawnPlayerEvent {
                        team,
                        is_queen,
                        player_controller: PlayerController::WebSocket(controller_update),
                        delay: 0.0,
                        start_invincible: false,
                    });
                    // Insert the created player and its gamepad to the hashmap of joined players
                    // Since uniqueness was already checked above, we can insert here unchecked
                    joined_websockets.0.insert(player_id);
                }
            }
        }
        Err(_) => (),
    }
}

fn disconnect(
    mut commands: Commands,
    action_query: Query<(
        Entity,
        &ActionState<Action>,
        &Player,
        Has<Berry>,
        &Transform,
        Option<&RidingOnShip>,
        &Team,
        Has<Queen>,
    )>,
    mut joined_gamepads: ResMut<JoinedGamepads>,
    asset_server: Res<AssetServer>,
    mut join_gates: Query<(Entity, &Team, &mut TextureAtlas), With<JoinGate>>,
) {
    for (
        player_entity,
        action_state,
        player,
        killed_has_berry,
        killed_player_transform,
        maybe_riding_on_ship,
        team,
        is_queen,
    ) in action_query.iter()
    {
        if action_state.pressed(&Action::Disconnect) {
            if let PlayerController::Gamepad(gamepad) = player.player_controller {
                joined_gamepads.0.remove(&gamepad);
            }
            remove_player(
                &mut commands,
                player_entity,
                killed_has_berry,
                killed_player_transform,
                &asset_server,
                maybe_riding_on_ship,
            );
            if is_queen {
                for (join_gate, join_gate_team, mut gate_sprite) in join_gates.iter_mut() {
                    if join_gate_team == team {
                        commands.entity(join_gate).remove::<Team>();
                        gate_sprite.index = GATE_NEUTRAL_IDX;
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn remove_player(
    commands: &mut Commands,
    player_entity: Entity,
    has_berry: bool,
    transform: &Transform,
    asset_server: &Res<AssetServer>,
    maybe_riding_on_ship: Option<&RidingOnShip>,
) {
    // Despawn the disconnected player and remove them from the joined player list
    commands.entity(player_entity).despawn_recursive();

    if has_berry {
        commands.spawn(BerryBundle::new(
            transform.translation.x,
            transform.translation.y,
            RigidBody::Dynamic,
            asset_server,
        ));
    }
    if let Some(riding_on_ship) = maybe_riding_on_ship {
        commands.entity(riding_on_ship.ship).remove::<Team>();
    }
}
