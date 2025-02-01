//! This file contains the Rustastic Drone implementation, developed by the Group Rustastic.
//!
//! File:   drone/drone.rs
//!
//! Brief:  Main file for the Rustastic Drone, containing the core drone logic.
//!
//! Author: Rustastic (Andrea Carzeri, Alessandro Busola, Andrea Denina, Giulio Bosio)

use colored::Colorize;
use crossbeam_channel::{select_biased, Receiver, Sender};
use log::{error, info, warn};
use rand::Rng;
use std::collections::{HashMap, HashSet};

use wg_2024::{
    controller::{DroneCommand, DroneEvent},
    drone::Drone,
    network::{NodeId, SourceRoutingHeader},
    packet::{FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType},
};

use crate::packet_buffer;

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone)]
/// A Rustastic drone entity.
///
/// The `RustasticDrone` struct represents a single drone with a unique ID,
/// communication channels for sending and receiving commands and packets,
/// and state for handling packet delivery and flood detection. It includes
/// functionality for packet routing, handling network events, and managing
/// the drone's internal packet buffer.
///
/// # Fields
/// - `id`: The unique identifier of the drone.
/// - `controller_send`: A channel for sending events to the controller.
/// - `controller_recv`: A channel for receiving commands from the controller.
/// - `packet_recv`: A channel for receiving incoming packets from other drones.
/// - `pdr`: The Packet Drop Rate (PDR), a float representing the probability
///   that a packet will be dropped during transmission.
/// - `packet_send`: A map that associates neighboring drone IDs to their packet-sending channels.
/// - `flood_id_received`: A set that caches flood IDs already processed, used to prevent
///   duplicate packet processing in the context of flood-based protocols.
/// - `buffer`: A packet buffer to temporarily store packets that pass through the drone.
pub struct RustasticDrone {
    id: NodeId,
    controller_send: Sender<DroneEvent>,
    controller_recv: Receiver<DroneCommand>,
    packet_recv: Receiver<Packet>,
    pdr: f32,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    flood_id_received: HashSet<(u64, NodeId)>, // Caching received flood_id
    pub buffer: packet_buffer::PacketBuffer,   // Packet buffer
}

impl Drone for RustasticDrone {
    /// Creates a new `RustasticDrone`.
    ///
    /// # Arguments
    /// - `id`: The unique identifier of the drone.
    /// - `controller_send`: The channel to send events to the controller.
    /// - `controller_recv`: The channel to receive commands from the controller.
    /// - `packet_recv`: The channel to receive packets from other drones.
    /// - `packet_send`: A map of packet-sending channels to other drones, keyed by their IDs.
    /// - `pdr`: The Packet Drop Rate of the drone, which affects transmission reliability.
    ///
    /// The field `flood_id_received` is initialized to an empty`HashSet`
    /// The field buffer is initialized with a `PacketBuffer` with default size of 16 packet
    ///
    /// # Returns
    /// A new instance of `RustasticDrone`.
    fn new(
        id: NodeId,
        controller_send: Sender<DroneEvent>,
        controller_recv: Receiver<DroneCommand>,
        packet_recv: Receiver<Packet>,
        packet_send: HashMap<NodeId, Sender<Packet>>,
        pdr: f32,
    ) -> Self {
        Self {
            id,
            controller_send,
            controller_recv,
            packet_recv,
            packet_send,
            pdr,
            flood_id_received: HashSet::new(),
            buffer: packet_buffer::PacketBuffer::new(16),
        }
    }

    /// Runs the main loop of the drone, continuously processing commands and packets.
    ///
    /// This function enters an infinite loop, constantly monitoring two channels:
    /// - `self.controller_recv`: Receives commands from the drone controller.
    /// - `self.packet_recv`: Receives raw data packets.
    ///
    /// The loop uses `select_biased!` to efficiently handle incoming data from both channels.
    ///
    /// Command from simulation controller are prioritized over data packets
    ///
    /// # Behavior:
    /// - If a `DroneCommand::Crash` is received, the loop terminates and a warning message is logged.
    /// - Other commands are passed to the `handle_command` function for further processing.
    /// - Received packets are passed to the `handle_packet` function for handling.
    fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.controller_recv) -> command => {
                    if let Ok(command) = command {
                        match command {
                            DroneCommand::Crash => {
                                warn!("{} [ Drone {} ]: Has crashed", "!!!".yellow(), self.id);
                                break;
                            },
                            _ => self.handle_command(command)
                        }
                    }
                }
                recv(self.packet_recv) -> packet => {
                    if let Ok(packet) = packet {
                        self.handle_packet(packet);
                    }
                },
            }
        }
    }
}

impl RustasticDrone {
    /// Handles incoming packets for the drone.
    ///
    /// This method is responsible for processing incoming packets. It checks the type of packet received and
    /// handles it accordingly. The packet could be a flood request, a normal message fragment, or an acknowledgment
    /// (either `Nack` or `Ack`). The method performs several checks to ensure that the packet is valid and
    /// can be forwarded to the correct next hop. If any errors are found, appropriate `Nack` packets are sent.
    ///
    /// # Packet Handling Logic
    /// - **`FloodRequest`**: If the packet is a flood request, it handles the request by calling `handle_flood_request`,
    ///   and then adds the flood ID to the `flood_id_received` set to prevent duplicate processing of the same flood.
    /// - **Correct Packet ID**: If the packet has the correct ID and is routable, it continues with routing and hop management.
    /// - **Destination Check**: If the destination of the packet is not a valid destination (e.g., a drone instead of a client/server),
    ///   it sends a `Nack` with an error message (`DestinationIsDrone`).
    /// - **Neighbor Check**: If the packet cannot be forwarded to a neighbor (i.e., the next hop is not in the drone's neighbor list),
    ///   it sends a `Nack` indicating an error in routing.
    /// - **Packet Type Handling**: Depending on the packet type, the method delegates the handling to the appropriate sub-methods:
    ///   - `Nack` and `Ack` packets are processed by `handle_ack_nack`.
    ///   - Fragmented messages are processed by `handle_fragment`.
    ///   - Flood responses are handled by `handle_flood_response`.
    ///
    /// # Arguments
    /// - `packet`: The incoming `Packet` that needs to be processed.
    ///
    /// # Behavior
    /// - The method first checks if the packet is a flood request, and handles it accordingly.
    /// - If the packet is a normal message, it checks the validity of the destination and the neighbors, then forwards it or
    ///   sends a `Nack` if needed.
    ///
    /// # Example
    /// ```rust
    /// // Assuming `packet` is a received packet to handle
    /// drone.handle_packet(packet);
    /// ```
    fn handle_packet(&mut self, mut packet: Packet) {
        info!(
            "{} [ Drone {} ]: has received the packet {:?}",
            "✓".green(),
            self.id,
            packet
        );

        if let PacketType::FloodRequest(flood_request) = packet.clone().pack_type {
            let flood_id = flood_request.flood_id;
            let flood_initiator = flood_request.initiator_id;
            self.handle_flood_request(flood_request, &packet);
            self.flood_id_received.insert((flood_id, flood_initiator));
        } else if self.check_packet_correct_id(packet.clone()) {
            // Increase hop_index
            packet.routing_header.increase_hop_index();

            // If the destination has been reached, and it is a Drone (invalid destination)
            if packet.routing_header.hop_index == packet.routing_header.hops.len() {
                error!(
                    "{} The selected destination in the RoutingHeader of [ Drone {} ] is a Drone",
                    "✗".red(),
                    self.id
                );
                self.send_nack(packet, None, NackType::DestinationIsDrone);
                return;
            }

            // Check if the next hop is a valid neighbor
            if !self.check_neighbor(&packet) {
                //Step4
                let neighbor = packet.routing_header.hops[packet.routing_header.hop_index];
                error!(
                    "{} [ Drone {} ]: can't send packet to Drone {} because it is not its neighbor",
                    "✗".red(),
                    self.id,
                    neighbor
                );
                self.send_nack(packet, None, NackType::ErrorInRouting(self.id));
                return;
            }

            info!(
                "{} Packet with [ session_id: {} ] is being handled from [ Drone {} ]",
                "✓".green(),
                packet.session_id,
                self.id
            );

            // Handle packet types: Nack, Ack, MsgFragment, FloodResponse
            match packet.clone().pack_type {
                PacketType::Nack(_nack) => self.handle_ack_nack(packet),
                PacketType::Ack(_ack) => self.handle_ack_nack(packet),
                PacketType::MsgFragment(fragment) => self.handle_fragment(packet, fragment),
                PacketType::FloodRequest(_) => unreachable!(),
                PacketType::FloodResponse(flood_response) => {
                    self.handle_flood_response(&flood_response, &packet);
                }
            }
        }
    }

    /// Sends a message packet to the destination drone, or forwards it to the simulation controller if an error occurs.
    ///
    /// This method is responsible for sending a `Packet` to the next drone in the routing path. It checks if the drone has a
    /// valid connection (through its `packet_send` map) to the destination. If the packet is successfully sent, it logs the
    /// event and returns `true`. If the destination is unreachable or an error occurs during sending, it logs the error
    /// and forwards the packet to the simulation controller, returning `false` to indicate failure.
    ///
    /// # Arguments
    /// - `packet`: The `Packet` to be sent. It contains the routing information and the packet type.
    ///
    /// # Return Value
    /// Returns a `bool`:
    /// - `true`: if the packet was successfully sent to the destination drone.
    /// - `false`: if there was an error in sending the packet, either due to an unreachable destination or a failure in the
    ///           communication channel.
    /// # Behavior
    /// - The method extracts the next hop in the routing path (`destination`), checks if the destination is available in
    ///   `packet_send`, and attempts to send the packet to that destination.
    /// - If the destination is unreachable, it checks if the packet is a `MsgFragment`. If so, it logs the failure without
    ///   attempting to send to the simulation controller. For other packet types, it forwards the packet to the simulation
    ///   controller with a `PacketDropped` event.
    ///
    /// # Example
    /// ```rust
    /// let packet = Packet { /* packet data */ };
    /// let sent = drone.send_message(packet);
    /// if sent {
    ///     println!("Message sent successfully!");
    /// } else {
    ///     println!("Failed to send the message.");
    /// }
    /// ```
    fn send_message(&self, packet: Packet) -> bool {
        let destination = packet.routing_header.hops[packet.routing_header.hop_index];
        let packet_type = packet.pack_type.clone();

        // Try sending to the destination drone
        if let Some(sender) = self.packet_send.get(&destination) {
            match sender.send(packet.clone()) {
                Ok(()) => {
                    info!(
                        "{} [ Drone {} ]: was sent a {} packet to [ Drone {} ]",
                        "✓".green(),
                        self.id,
                        packet_type,
                        destination
                    );
                    true
                }
                Err(e) => {
                    // In case of an error, forward the packet to the simulation controller
                    error!(
                        "{} [ Drone {} ]: Failed to send the {} to [ Drone {} ]: {}",
                        "✗".red(),
                        self.id,
                        packet_type,
                        destination,
                        e
                    );

                    warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

                    self.controller_send
                        .send(DroneEvent::PacketDropped(packet))
                        .unwrap();

                    warn!(
                        "└─>{} [ Drone {} ]: {} sent to Simulation Controller",
                        "!!!".yellow(),
                        self.id,
                        packet_type,
                    );

                    false
                }
            }
        } else {
            // Handle case where there is no connection to the destination drone
            if let PacketType::MsgFragment(_) = packet_type {
                error!(
                    "{} [ Drone {} ]: does not exist in the path",
                    "✗".red(),
                    destination
                );
            } else {
                error!(
                    "{} [ Drone {} ]: Failed to send the {}: No connection to [ Drone {} ]",
                    "✗".red(),
                    self.id,
                    packet_type,
                    destination
                );

                warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

                self.controller_send
                    .send(DroneEvent::PacketDropped(packet))
                    .unwrap();

                warn!(
                    "└─>{} [ Drone {} ]: {} sent to Simulation Controller",
                    "!!!".yellow(),
                    self.id,
                    packet_type,
                );
            }

            false
        }
    }

    /// Checks if the drone's ID matches the expected recipient ID in the packet's routing header.
    ///
    /// This method compares the current drone's ID with the ID specified in the packet's routing header at the
    /// current hop index. If the IDs match, the method returns `true`, indicating that the packet is addressed to
    /// this drone. If the IDs do not match, the method returns `false`, sends a NACK to the previous drone indicating
    /// an unexpected recipient, and logs an error.
    ///
    /// # Arguments
    /// - `packet`: The `Packet` that contains the routing header, which includes a list of hops and the current hop index.
    ///
    /// # Return Value
    /// Returns a `bool`:
    /// - `true`: if the current drone's ID matches the recipient ID in the packet's routing header.
    /// - `false`: if the current drone's ID does not match the recipient ID, indicating an incorrect recipient.
    ///
    /// # Behavior
    /// - The method compares the drone's ID (`self.id`) with the destination ID in the packet's routing header,
    ///   at the position indicated by `hop_index`.
    /// - If the IDs match, the method returns `true`, confirming the packet is intended for this drone.
    /// - If the IDs do not match, it sends a NACK with the error type `UnexpectedRecipient`, logs the mismatch error,
    ///   and returns `false`.
    ///
    /// # Example
    /// ```rust
    /// let packet = Packet { /* packet data */ };
    /// let is_correct = drone.check_packet_correct_id(packet);
    /// if is_correct {
    ///     println!("Packet addressed correctly to the drone.");
    /// } else {
    ///     println!("Packet addressed to the wrong drone.");
    /// }
    /// ```
    fn check_packet_correct_id(&self, packet: Packet) -> bool {
        if self.id == packet.routing_header.hops[packet.routing_header.hop_index] {
            true
        } else {
            self.send_nack(packet, None, NackType::UnexpectedRecipient(self.id));
            error!(
                "{} [ Drone {} ]: does not correspond to the Drone indicated by the `hop_index`",
                "✗".red(),
                self.id
            );
            false
        }
    }

    /// Handles the reception of ACK and NACK packets, managing fragment retransmissions and routing updates.
    ///
    /// This method processes incoming NACK or ACK packets, depending on the packet type. If a NACK is received, it attempts
    /// to find the corresponding fragment in the drone's buffer and resends it. If the fragment is not found, a NACK is sent
    /// back to the previous hop. For ACK packets, the method ensures the correct routing and either forwards the packet or
    /// updates the routing header accordingly.
    ///
    /// # Arguments
    /// - `packet`: The `Packet` that contains the routing header and packet type (NACK, ACK, etc.) to be processed.
    ///
    /// # Behavior
    /// - **If the packet type is a NACK**:
    ///   - It checks the drone's buffer for the requested fragment using the `session_id` and `fragment_index`.
    ///   - If the fragment is found, it is resent by reversing the routing path and creating a new packet with the fragment.
    ///   - If the fragment is not found, a NACK is sent to the previous node, indicating that the fragment could not be found.
    /// - **If the packet type is an ACK**:
    ///   - It increments the hop index and forwards the packet to the next hop, if the index is valid.
    ///   - If the hop index is at the end of the route, it logs an error.
    ///
    /// # Panic
    /// - If the hop index is greater than or equal to the number of hops, it causes a panic with the message "Source is not a client!".
    ///
    /// # Example
    /// ```rust
    /// let packet = Packet { /* packet data */ };
    /// drone.handle_ack_nack(packet);
    /// ```
    fn handle_ack_nack(&mut self, mut packet: Packet) {
        if packet.routing_header.hop_index >= packet.routing_header.hops.len() {
            // It can't happen
            unreachable!("{} Source is not a client!", "PANIC".purple());
        }
        if let PacketType::Nack(nack) = packet.clone().pack_type {
            warn!(
                "{} [ Drone {} ]: received a {}",
                "!!!".yellow(),
                self.id,
                packet.pack_type,
            );

            warn!(
                "\n├─>{} Checking [ Drone {} ] buffer...",
                "!!!".yellow(),
                self.id
            );

            // Check if the fragment is in the buffer
            if let Some(fragment) = self
                .buffer
                .get_fragment(packet.clone().session_id, nack.fragment_index)
            {
                info!(
                    "├─>{} Fragment [ fragment_index: {} ] of the Packet [ session_id: {} ]  was found in the buffer",
                    "✓".green(),
                    fragment.fragment_index,
                    packet.session_id
                );

                // Resend the fragment, reverse the path
                packet.routing_header.hop_index -= 1;
                packet.routing_header.reverse();
                packet.routing_header.hop_index += 1;
                let new_packet = Packet {
                    pack_type: PacketType::MsgFragment(fragment.clone()),
                    routing_header: packet.routing_header.clone(),
                    session_id: packet.session_id,
                };
                self.send_message(new_packet);

                info!("└─>{} The Packet was sent", "✓".green());
            } else {
                // Send a nack to the previous node
                self.send_message(packet);
            }
        } else {
            if packet.routing_header.hop_index == packet.routing_header.hops.len() - 1 {
                error!(
                    "{} Invalid hop index increment detected in [ Drone: {} ] for header of Packet [ session_id: {} ]",
                    "✗".red(),
                    self.id,
                    packet.session_id
                );
                return;
            }
            self.send_message(packet);
        }
    }

    /// Handles the reception of a fragmented packet, either dropping it based on the Packet Drop Rate (PDR)
    /// or forwarding and storing it in the drone's buffer.
    ///
    /// This method processes a packet fragment by either dropping it, based on the drone's PDR, or forwarding it
    /// to the next hop. If the fragment is not dropped, it is added to the drone's buffer to be potentially retransmitted
    /// later. If the fragment is dropped, a NACK is sent to notify the sender of the packet drop.
    ///
    /// # Arguments
    /// - `packet`: The `Packet` containing the routing header and session ID, which the fragment belongs to.
    /// - `fragment`: The `Fragment` that is part of the `packet` and contains the fragmented data.
    ///
    /// # Behavior
    /// - **If the fragment is dropped** (based on the `check_drop_fragment` method):
    ///   - A message is printed to indicate that the fragment was dropped by the drone.
    ///   - A NACK is sent to the previous node, indicating that the fragment was dropped.
    /// - **If the fragment is not dropped**:
    ///   - The fragment is added to the drone's buffer using its `session_id` and the fragment's `fragment_index`.
    ///   - A message is printed to indicate that the fragment was successfully added to the buffer.
    ///   - The `packet` is forwarded to the next hop by calling `send_message`.
    ///
    /// # Example
    /// ```rust
    /// let packet = Packet { /* packet data */ };
    /// let fragment = Fragment { /* fragment data */ };
    /// drone.handle_fragment(packet, fragment);
    /// ```
    fn handle_fragment(&mut self, packet: Packet, fragment: Fragment) {
        if self.check_drop_fragment() {
            warn!(
                "{} Fragment [ fragment_index: {} ] of the Packet [ session_id: {} ] has been dropped by [ Drone {} ]",
                "!!!".yellow(),
                fragment.fragment_index,
                packet.session_id,
                self.id
            );
            self.send_nack(packet, Some(fragment), NackType::Dropped);
        } else {
            // Add the fragment to the buffer
            info!(
                "{} [ Drone {} ]: forwarded the the fragment [ fragment_index: {} ] of the Packet [ session_id: {} ]",
                "✓".green(),
                self.id,
                fragment.fragment_index,
                packet.session_id
            );

            self.buffer
                .add_fragment(packet.clone().session_id, fragment);
            self.send_message(packet);

            warn!(
                "└─>{} Fragment was added to the [ Drone {} ] buffer",
                "!!!".yellow(),
                self.id
            );
        }
    }

    /// Sends a NACK (Negative Acknowledgment) to the previous hop or to the simulation controller in case of an error.
    ///
    /// This function sends a NACK message back to the previous drone in the routing path when there is a problem with
    /// the packet or its fragment, such as a routing error or dropped packet. If the NACK cannot be sent to the
    /// previous drone due to an issue (e.g., no connection), the NACK is sent to the simulation controller to notify
    /// of the failure.
    ///
    /// # Arguments
    /// - `packet`: The `Packet` that needs to be acknowledged (or nacked). This packet is either a regular packet or a fragment of a larger message.
    /// - `fragment`: An optional `Fragment` object that contains specific fragment data, if the NACK is related to a dropped fragment.
    /// - `nack_type`: The `NackType` representing the type of error or problem that occurred. This can be a dropped packet, an unexpected recipient, etc.
    ///
    /// # Behavior
    /// - The function reverses the routing header to determine the previous hop in the routing path and attempts to send the NACK to that drone.
    /// - If the NACK is related to a fragment (i.e., the `fragment` argument is `Some`), the function updates the NACK's fragment index and type accordingly.
    /// - If the NACK is successfully sent to the previous drone, a message is logged to indicate the successful transmission.
    /// - If the drone cannot send the NACK to the previous hop (e.g., no connection), it sends the NACK to the simulation controller and logs the event.
    ///
    /// # Example
    /// ```rust
    /// let packet = Packet { /* packet data */ };
    /// let fragment = Some(Fragment { /* fragment data */ });
    /// let nack_type = NackType::Dropped;
    /// drone.send_nack(packet, fragment, nack_type);
    /// ```
    fn send_nack(&self, mut packet: Packet, fragment: Option<Fragment>, nack_type: NackType) {
        // Reverse the routing header to get the previous hop
        packet.routing_header.hop_index -= 1;
        packet.routing_header.reverse();
        let prev_hop = packet.routing_header.next_hop().unwrap();
        packet.routing_header.increase_hop_index();
        // Attempt to send the NACK to the previous hop
        if let Some(sender) = self.packet_send.get(&prev_hop) {
            let mut nack = Nack {
                fragment_index: 0, // Default fragment index for non-fragmented NACKs
                nack_type,
            };
            // If it's a fragment, set the fragment index and NACK type accordingly
            if let Some(frag) = fragment {
                nack = Nack {
                    fragment_index: frag.fragment_index,
                    nack_type: NackType::Dropped,
                };
            }

            // Update the packet type to NACK
            packet.pack_type = PacketType::Nack(nack);

            // Send the NACK to the previous hop
            match sender.send(packet.clone()) {
                Ok(()) => {
                    warn!(
                        "{} Nack was sent from [ Drone {} ] to [ Drone {} ]",
                        "!!!".yellow(),
                        self.id,
                        prev_hop
                    );
                }
                Err(e) => {
                    // Handle failure to send the NACK, send to the simulation controller instead
                    warn!(
                        "{} [ Drone {} ]: Failed to send the Nack to [ Drone {} ]: {}",
                        "✗".red(),
                        self.id,
                        prev_hop,
                        e
                    );

                    warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

                    //there is an error in sending the packet, the drone should send the packet to the simulation controller
                    self.controller_send
                        .send(DroneEvent::PacketDropped(packet))
                        .unwrap();
                    warn!(
                        "└─>{} [ Drone {} ]: sent A Nack to the Simulation Controller",
                        "!!!".yellow(),
                        self.id
                    );
                }
            }
        } else {
            // If no connection to the previous hop, send the NACK to the simulation controller
            error!(
                "{} [ Drone {} ]: Failed to send the Nack: No connection to {}",
                "✗".red(),
                self.id,
                prev_hop
            );

            warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

            // Create the NACK (same logic as above)
            let mut nack = Nack {
                fragment_index: 0,
                nack_type,
            };

            if let Some(frag) = fragment {
                nack = Nack {
                    fragment_index: frag.fragment_index,
                    nack_type: NackType::Dropped,
                };
            }
            packet.pack_type = PacketType::Nack(nack);

            // Send to the simulation controller
            self.controller_send
                .send(DroneEvent::PacketDropped(packet))
                .unwrap();
            warn!(
                "└─>{} [ Drone {} ]: sent A Nack to the Simulation Controller",
                "!!!".yellow(),
                self.id
            );
        }
    }

    /// Checks if the destination of a packet is a neighboring drone.
    ///
    /// This function checks whether the destination drone, as indicated in the packet's routing header,
    /// is a valid neighbor of the current drone. It does so by verifying if there is a connection to
    /// the destination drone in the `packet_send` map, which holds the communication channels for sending
    /// packets to neighboring drones.
    ///
    /// # Arguments
    /// - `packet`: The `Packet` whose destination is being checked. The packet contains a routing header
    ///   that specifies the destination drone and the current hop index.
    ///
    /// # Returns
    /// - `true` if the destination of the packet is a valid neighbor of the current drone (i.e., there is
    ///   a communication channel to the destination).
    /// - `false` if the destination is not a neighbor (i.e., there is no communication channel to the destination).
    ///
    /// # Example
    /// ```rust
    /// let packet = Packet { /* packet data */ };
    /// let is_neighbor = drone.check_neighbor(packet);
    /// if is_neighbor {
    ///     println!("The destination is a neighbor.");
    /// } else {
    ///     println!("The destination is not a neighbor.");
    /// }
    /// ```
    fn check_neighbor(&self, packet: &Packet) -> bool {
        let destination = packet.routing_header.hops[packet.routing_header.hop_index];
        self.packet_send.contains_key(&destination)
    }

    // Determines if a packet fragment should be dropped based on the Packet Drop Rate (PDR).
    ///
    /// This function simulates the packet drop behavior based on the current drone's Packet Drop Rate (PDR).
    /// It generates a random number between 1 and 100 and compares it to the scaled PDR (multiplied by 100).
    /// If the random value is less than or equal to the scaled PDR, the fragment is considered to be dropped.
    ///
    /// # Returns
    /// - `true` if the packet fragment should be dropped based on the current PDR.
    /// - `false` if the packet fragment should not be dropped.
    ///
    /// # Example
    /// ```rust
    /// let should_drop = drone.check_drop_fragment();
    /// if should_drop {
    ///     println!("The packet fragment will be dropped.");
    /// } else {
    ///     println!("The packet fragment will not be dropped.");
    /// }
    /// ```
    fn check_drop_fragment(&self) -> bool {
        let mut rng = rand::thread_rng();
        let val = rng.gen_range(1f32..=100f32);
        val <= self.pdr * 100f32
    }

    /// Handles an incoming `FloodRequest` packet and processes it accordingly.
    ///
    /// This function handles the logic for processing a `FloodRequest` packet that has been received by the drone.
    /// It checks if the flood request has already been received, processes the path trace, and either forwards
    /// the flood request to neighbors or responds with a `FloodResponse`. The flood request is a type of routing
    /// protocol used to propagate information across the network of drones, and the drone can either forward or
    /// respond based on its state.
    ///
    /// # Arguments
    /// - `flood_request`: The `FloodRequest` packet containing information about the flood and its path trace.
    /// - `packet`: The full packet that contains the flood request and additional metadata, such as the routing header.
    ///
    /// # Behavior:
    /// - The function first determines the last node in the path trace to identify the drone that sent the request.
    /// - It then checks if the flood request has already been received based on its `flood_id` and `initiator_id`.
    /// - If the request has already been processed, the drone sends a `FloodResponse` back to the previous node
    ///   indicating that the flood request has already been received.
    /// - If the drone has no neighbors to forward the request to (except the previous node), it sends a `FloodResponse`
    ///   to the previous node indicating that no further hops are available.
    /// - If the drone has neighbors to forward the request to, it sends the `FloodRequest` to each neighbor except
    ///   the one that sent the request (the `prev_node`).
    ///
    /// # Example:
    /// ```rust
    /// drone.handle_flood_request(flood_request, packet);
    /// ```
    fn handle_flood_request(&self, mut flood_request: FloodRequest, packet: &Packet) {
        // Determine the previous node that sent the packet
        let prev_node = if let Some(node) = flood_request.path_trace.last() {
            node.0
        } else {
            error!("A drone can't be the first node in the path-trace.");
            return;
        };

        // Add the current drone to the path-trace
        flood_request.path_trace.push((self.id, NodeType::Drone));

        // Check if the flood request has already been processed
        if self
            .flood_id_received
            .contains(&(flood_request.flood_id, flood_request.initiator_id))
        {
            // If it has been processed, send a FloodResponse to the previous node

            let mut new_hops: Vec<u8> = flood_request
                .clone()
                .path_trace
                .into_iter()
                .map(|(id, _ntype)| id)
                .collect();

            new_hops.reverse();

            let new_routing_header = SourceRoutingHeader {
                hop_index: 1,
                hops: new_hops,
            };

            // Send the FloodResponse to the previous node
            self.send_flood_response(
                prev_node,
                &flood_request,
                new_routing_header,
                packet.session_id,
                format!(
                    "{} [ Drone {} ]: has already received a FloodRequest with flood_id: {}",
                    "!!!".yellow(),
                    self.id,
                    flood_request.flood_id
                )
                .as_str(),
            );
        } else if self.packet_send.len() == 1 {
            // If the drone has no neighbors except the previous node
            let mut new_hops: Vec<u8> = flood_request
                .clone()
                .path_trace
                .into_iter()
                .map(|(id, _ntype)| id)
                .collect();

            new_hops.reverse();

            let new_routing_header = SourceRoutingHeader {
                hop_index: 1,
                hops: new_hops,
            };

            // Send a FloodResponse indicating no further neighbors to forward the request
            self.send_flood_response(
                prev_node,
                &flood_request,
                new_routing_header,
                packet.session_id,
                format!(
                    "{} [ Drone {} ]: doesn't have any other neighbors to send the FloodRequest with flood_id: {}",
                    "!!!".yellow(),
                    self.id,
                    flood_request.flood_id
                )
                    .as_str(),
            );
        } else {
            // Forward the FloodRequest to all neighbors except the previous node
            for neighbor in self
                .packet_send
                .iter()
                .filter(|neighbor| *neighbor.0 != prev_node)
            {
                self.send_flood_request(
                    neighbor,
                    &flood_request,
                    packet.routing_header.clone(),
                    packet.session_id,
                );

                info!(
                    "{} [ Drone {} ]: sent a FloodRequest with flood_id: {} to the [ Drone {} ]",
                    "✓".green(),
                    self.id,
                    flood_request.flood_id,
                    neighbor.0
                );
            }
        }
    }

    /// Handles the incoming `FloodResponse` packet, processes it, and sends it back to the appropriate drone.
    ///
    /// This function processes a received `FloodResponse` packet by forwarding it to the next hop in the path.
    /// If the next hop is unavailable or if an error occurs while sending the response, the packet is sent to the
    /// simulation controller instead. It plays a critical role in routing flood responses back through the network
    /// of drones.
    ///
    /// # Arguments
    /// - `flood_response`: The `FloodResponse` packet that was received. This packet contains the information
    ///   about the flood response and its associated metadata.
    /// - `packet`: The full packet, which contains additional information like the routing header and session ID.
    ///
    /// # Behavior:
    /// - The function attempts to forward the `FloodResponse` to the next drone in the path specified by the
    ///   `routing_header` in the `packet`.
    /// - If the next hop is available in `packet_send`, the packet is forwarded to the next drone.
    /// - If the next hop is not available, or if there is an error in sending the packet, the packet is sent to
    ///   the simulation controller to handle the issue.
    ///
    /// # Example:
    /// ```rust
    /// drone.handle_flood_response(flood_response, packet);
    /// ```
    fn handle_flood_response(&self, flood_response: &FloodResponse, packet: &Packet) {
        let new_routing_header = packet.routing_header.clone();

        // Prepare a new packet to send the flood response back
        let new_packet = Packet {
            pack_type: PacketType::FloodResponse(flood_response.clone()),
            routing_header: new_routing_header.clone(),
            session_id: packet.session_id,
        };

        // Try to send the FloodResponse to the next hop in the routing path
        if let Some(sender) = self
            .packet_send
            .get(&new_routing_header.hops[new_routing_header.hop_index])
        {
            match sender.send(new_packet.clone()) {
                Ok(()) => info!(
                    "{} [ Drone {} ]: sent a FloodResponse with flood_id: {} to [ Drone {} ]",
                    "✓".green(),
                    self.id,
                    flood_response.flood_id,
                    &new_routing_header.hops[new_routing_header.hop_index]
                ),
                Err(e) => {
                    error!(
                        "{} [ Drone {} ]: failed to send the FloodResponse to the Drone {}: {}",
                        "✗".red(),
                        self.id,
                        &new_routing_header.hops[new_routing_header.hop_index],
                        e
                    );

                    warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

                    self.controller_send
                        .send(DroneEvent::PacketDropped(new_packet))
                        .unwrap();

                    warn!(
                        "└─>{} [ Drone {} ]: sent the FloodResponse to the Simulation Controller",
                        "!!!".yellow(),
                        self.id
                    );
                }
            }
        } else {
            // If the next hop is unavailable, send the packet to the simulation controller
            error!(
                "{} [ Drone {} ]: failed to send the FloodResponse: No connection to [ Drone {} ]",
                "✗".red(),
                self.id,
                &new_routing_header.hops[new_routing_header.hop_index]
            );

            warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

            // Send the packet to the simulation controller
            self.controller_send
                .send(DroneEvent::PacketDropped(new_packet))
                .unwrap();

            warn!(
                "└─>{} [ Drone {} ]: sent the FloodResponse to the Simulation Controller",
                "!!!".yellow(),
                self.id
            );
        }
    }

    /// Sends a `FloodRequest` packet to a specified destination drone.
    ///
    /// This function creates a new `FloodRequest` packet and sends it to the specified destination drone.
    /// The `FloodRequest` contains information about the flood ID, initiator ID, and path trace. The packet
    /// is routed according to the provided `routing_header` and `session_id`.
    ///
    /// # Arguments
    /// - `dest_node`: A tuple containing the destination drone's ID (`NodeId`) and the corresponding sender
    ///   (`Sender<Packet>`) to send the packet to.
    /// - `flood_request`: The `FloodRequest` that is being sent, containing the flood ID, initiator ID, and the
    ///   path trace up to this point in the flood.
    /// - `routing_header`: The `SourceRoutingHeader` which provides routing information for this packet.
    /// - `session_id`: A unique session ID that helps track the packet across the network.
    ///
    /// # Behavior:
    /// - The function creates a new `FloodRequest` packet based on the provided `flood_request` and `routing_header`.
    /// - It then attempts to send this packet to the specified destination drone using the provided sender (`Sender<Packet>`).
    /// - If sending the packet is successful, a success message is logged. If there is an error, an error message is logged.
    ///
    /// # Example:
    /// ```rust
    /// drone.send_flood_request((&destination_id, &destination_sender), &flood_request, routing_header, session_id);
    /// ```
    fn send_flood_request(
        &self,
        dest_node: (&NodeId, &Sender<Packet>),
        flood_request: &FloodRequest,
        routing_header: SourceRoutingHeader,
        session_id: u64,
    ) {
        let flood_id = flood_request.flood_id;
        let new_flood_request = FloodRequest {
            flood_id,
            initiator_id: flood_request.initiator_id,
            path_trace: flood_request.path_trace.clone(),
        };

        let new_packet = Packet {
            pack_type: PacketType::FloodRequest(new_flood_request),
            routing_header,
            session_id,
        };

        match dest_node.1.send(new_packet) {
            Ok(()) => info!(
                "{} [ Drone {} ]: sent the FloodRequest with flood_id: {} sent to [ Drone {} ]",
                "✓".green(),
                self.id,
                flood_id,
                dest_node.0
            ),
            Err(e) => error!(
                "{} [ Drone {} ]: failed to send FloodRequest to the [ Drone {} ]: {}",
                "✗".red(),
                self.id,
                dest_node.0,
                e
            ),
        };
    }

    /// Sends a `FloodResponse` packet to a specified destination drone.
    ///
    /// This function creates a `FloodResponse` packet and sends it to a destination drone. The response includes
    /// the flood ID and the path trace that was followed during the flood request. The packet is routed according
    /// to the provided `routing_header` and is identified by the provided `session_id`.
    ///
    /// # Arguments
    /// - `dest_node`: The ID of the destination drone to send the response to.
    /// - `flood_request`: The `FloodRequest` that prompted this response. It contains the `flood_id` and `path_trace`.
    /// - `routing_header`: The `SourceRoutingHeader` containing routing information for the packet.
    /// - `session_id`: A unique session ID used to track the packet across the network.
    /// - `reason`: A description or reason for sending the response. This will be logged along with the action.
    ///
    /// # Behavior:
    /// - The function creates a `FloodResponse` packet based on the `flood_request` and the `routing_header`.
    /// - The response is then sent to the destination drone using the provided `dest_node`.
    /// - If the packet is successfully sent, a success message is logged. If there is an error in sending the packet,
    ///   the packet is sent to the simulation controller to indicate a failure.
    ///
    /// # Example:
    /// ```rust
    /// drone.send_flood_response(destination_id, &flood_request, routing_header, session_id, "FloodRequest already received");
    /// ```
    fn send_flood_response(
        &self,
        dest_node: NodeId,
        flood_request: &FloodRequest,
        routing_header: SourceRoutingHeader,
        session_id: u64,
        reason: &str,
    ) {
        let flood_response = FloodResponse {
            flood_id: flood_request.flood_id,
            path_trace: flood_request.path_trace.clone(),
        };

        let new_packet = Packet {
            pack_type: PacketType::FloodResponse(flood_response.clone()),
            routing_header,
            session_id,
        };

        if let Some(sender) = self.packet_send.get(&dest_node) {
            match sender.send(new_packet.clone()) {
                Ok(()) => info!(
                    "{} [ Drone {} ]: sent the FloodResponse to [ Drone {} ]\n└─>Reason: {}",
                    "✓".green(),
                    self.id,
                    dest_node,
                    reason
                ),
                Err(e) => {
                    error!(
                        "{} [ Drone {} ]: Failed to send the FloodResponse to [ Drone {} ]: {}",
                        "✗".red(),
                        self.id,
                        dest_node,
                        e
                    );

                    warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

                    self.controller_send
                        .send(DroneEvent::PacketDropped(new_packet))
                        .unwrap();

                    warn!(
                        "└─>{} [ Drone {} ]: FloodResponse sent to Simulation Controller",
                        "!!!".yellow(),
                        self.id
                    );
                }
            }
        } else {
            // Handle the case where there is no connection to the destination drone
            error!(
                "{} [ Drone {} ]: Failed to send the FloodResponse: No connection to [ Drone {} ]",
                "✗".red(),
                self.id,
                dest_node
            );

            warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

            self.controller_send
                .send(DroneEvent::PacketDropped(new_packet))
                .unwrap();

            warn!(
                "└─>{} [ Drone {} ]: FloodResponse sent to Simulation Controller",
                "!!!".yellow(),
                self.id
            );
        }
    }

    /// Handles incoming drone commands to manage network connections and settings.
    ///
    /// This function processes different types of commands for the drone, including adding or removing senders,
    /// adjusting the packet drop rate (PDR), and handling a crash command. Based on the command type, the drone
    /// either updates its network connections, modifies its settings, or triggers the respective behavior.
    ///
    /// # Arguments
    /// - `command`: The drone command to handle, which can be one of the following:
    ///   - `AddSender(node_id, sender)`: Adds a sender (a connection to another drone) to the drone's network.
    ///   - `SetPacketDropRate(pdr)`: Sets the packet drop rate (PDR) of the drone, controlling the likelihood of
    ///     dropping packets during transmission.
    ///   - `RemoveSender(node_id)`: Removes a sender (a connection to another drone) from the drone's network.
    ///   - `Crash`: This command simulates a crash of the drone, but it is marked as `unreachable` because it is
    ///     not meant to be processed here.
    ///
    /// # Behavior:
    /// - **`AddSender`**: Adds the sender to the drone's list of connected drones if not already connected.
    /// - **`SetPacketDropRate`**: Sets the drone’s packet drop rate (PDR), ensuring the value is between `0.0` and `1.0`.
    /// - **`RemoveSender`**: Removes the sender from the drone's list of connected drones if it exists.
    /// - **`Crash`**: This command is considered unreachable in the current context and will not be processed.
    ///
    /// # Panic:
    /// - Panic if a command of type `DroneCommand::Crash` is passed
    ///
    /// # Example:
    /// ```rust
    /// drone.handle_command(DroneCommand::AddSender(node_id, sender));
    /// drone.handle_command(DroneCommand::SetPacketDropRate(0.1));
    /// drone.handle_command(DroneCommand::RemoveSender(node_id));
    /// ```
    fn handle_command(&mut self, command: DroneCommand) {
        match command {
            DroneCommand::AddSender(node_id, sender) => {
                if let std::collections::hash_map::Entry::Vacant(e) =
                    self.packet_send.entry(node_id)
                {
                    info!(
                        "{} Adding sender: {} to [ Drone {} ]",
                        "✓".green(),
                        node_id,
                        self.id,
                    );
                    e.insert(sender);
                } else {
                    warn!(
                        "{} [ Drone {} ] is already connected to [ Drone {} ]",
                        "!!!".yellow(),
                        self.id,
                        node_id
                    );
                }
            }
            DroneCommand::SetPacketDropRate(pdr) => {
                if (0.0..=1.0).contains(&pdr) {
                    info!(
                        "{} Setting [ Drone {} ] pdr to: {}",
                        "✓".green(),
                        self.id,
                        pdr
                    );
                    self.pdr = pdr;
                } else {
                    error!(
                        "{} The pdr is a value that must be between `0.0` and `1.0`",
                        "✗".red()
                    );
                }
            }
            DroneCommand::RemoveSender(node_id) => {
                if self.packet_send.contains_key(&node_id) {
                    info!(
                        "{} Removing sender: {} from [ Drone {} ]",
                        "✓".green(),
                        node_id,
                        self.id
                    );
                    self.packet_send.remove(&node_id);
                } else {
                    warn!(
                        "{} [ Drone {} ] is already disconnected from [ Drone {} ]",
                        "!!!".yellow(),
                        self.id,
                        node_id
                    );
                }
            }
            DroneCommand::Crash => unreachable!(),
        }
    }
}
