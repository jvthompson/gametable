//! The table's backends for networked games: hosting (the authoritative session inside a host
//! engine, serving the players connected to a [`HostServer`]) and joining (a replicated session
//! whose actions go to the host as request messages). Ports of `controller.Host` with its preview
//! sink and `controller.Client`, plus the host engine wiring of `HostGame` in
//! `src/ui/screens/multiplayer.lua`.

use std::cell::RefCell;
use std::rc::Rc;

use gametable_core::host_engine::HostEngine;
use gametable_core::session::Session;

use crate::net::game_connection::GameConnection;
use crate::net::host_server::{HostServer, HostServerEvent};
use crate::table::actions::{TableAction, TableBackend, apply_as_host, request_message};

/// The host's side of a networked game.
pub struct NetworkHostBackend {
    pub engine: HostEngine,
    pub server: Rc<RefCell<HostServer>>,
}

impl NetworkHostBackend {
    /// `E:start()`: queues the server's events for the engine and sends everyone the state.
    pub fn new(engine: HostEngine, server: Rc<RefCell<HostServer>>) -> NetworkHostBackend {
        let mut backend = NetworkHostBackend { engine, server };
        {
            let mut server = backend.server.borrow_mut();
            server.collect_events = true;
            backend.engine.start(&mut *server);
        }
        backend
    }

    /// Lets disconnected players reclaim their seats.
    fn refresh_reclaimable_seats(&mut self) {
        let disconnected: Vec<String> = self
            .engine
            .session
            .state()
            .players
            .iter()
            .filter(|player| !player.connected)
            .map(|player| player.id.clone())
            .collect();
        self.server
            .borrow_mut()
            .set_reclaimable_check(Some(Box::new(move |id| disconnected.iter().any(|seat| seat == id))));
    }

    /// `E:dispose()`: stops queueing events.
    pub fn dispose(&mut self) {
        let mut server = self.server.borrow_mut();
        server.collect_events = false;
        server.set_reclaimable_check(None);
    }
}

impl TableBackend for NetworkHostBackend {
    fn session(&self) -> &Session {
        &self.engine.session
    }

    fn session_mut(&mut self) -> &mut Session {
        &mut self.engine.session
    }

    fn perform(&mut self, action: TableAction) {
        let acting = self.engine.session.acting_player_id().to_string();
        let mut server = self.server.borrow_mut();
        match action {
            TableAction::PreviewCardDrag { primary_instance_id, passenger_root_ids, fraction_x, fraction_y } => {
                let mut ids = vec![primary_instance_id];
                ids.extend(passenger_root_ids);
                self.engine.publish_card_drag_preview(&acting, ids, Some(fraction_x), Some(fraction_y), &mut *server);
            }
            TableAction::EndCardDragPreview => self.engine.clear_card_drag_preview(&acting, &mut *server),
            TableAction::PreviewArrowDrag { fraction_x, fraction_y, fraction_x2, fraction_y2 } => {
                let corners = (Some(fraction_x), Some(fraction_y), Some(fraction_x2), Some(fraction_y2));
                self.engine.publish_arrow_drag_preview(&acting, corners, &mut *server);
            }
            TableAction::EndArrowDragPreview => self.engine.clear_arrow_drag_preview(&acting, &mut *server),
            action => {
                apply_as_host(&mut self.engine.session, action);
                self.engine.broadcast_if_notified(&mut *server);
            }
        }
    }

    /// Pumps the server, hands its messages and roster changes to the engine, runs timers.
    fn update(&mut self, frame_index: u64) {
        self.refresh_reclaimable_seats();
        let mut server = self.server.borrow_mut();
        server.update(frame_index);
        for event in server.take_events() {
            match event {
                HostServerEvent::Incoming { player_id, message } => {
                    self.engine.handle_message(&player_id, &message.message_type, &message.payload, &mut *server);
                }
                HostServerEvent::RosterChanged => {
                    let roster = server.roster();
                    self.engine.on_roster(&roster, &mut *server);
                }
            }
        }
        self.engine.tick(&mut *server);
    }
}

/// A joined player's side: the session mirrors the host's state; actions become requests.
pub struct ClientBackend {
    pub session: Session,
    pub client: Rc<RefCell<dyn GameConnection>>,
}

impl TableBackend for ClientBackend {
    fn session(&self) -> &Session {
        &self.session
    }

    fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    fn perform(&mut self, action: TableAction) {
        let (message_type, payload) = request_message(&action);
        self.client.borrow_mut().send(message_type, payload);
    }
}
