use colored::Colorize;
use crossbeam_channel::{select_biased, Receiver, Sender};
use rand::Rng;
use std::collections::{HashMap, HashSet};

use wg_2024::{
    controller::{DroneCommand, DroneEvent},
    drone::Drone,
    network::{NodeId, SourceRoutingHeader},
    packet::{FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType},
};

mod packet_buffer;

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone)]
pub struct RustasticDrone {
    id: NodeId,
    controller_send: Sender<DroneEvent>,
    controller_recv: Receiver<DroneCommand>,
    packet_recv: Receiver<Packet>,
    pdr: f32,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    //caching the flood_id s already received
    flood_id_received: HashSet<(u64, NodeId)>,
    pub buffer: packet_buffer::PacketBuffer,
}

impl Drone for RustasticDrone {
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

    fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.controller_recv) -> command => {
                    if let Ok(command) = command {
                        match command {
                            DroneCommand::Crash => {
                                println!("{} [ Drone {} ]: Has crashed", "!!!".yellow(), self.id);
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
//spostare la logica di routing al drone
impl RustasticDrone {
    #[allow(clippy::too_many_lines)]
    fn handle_packet(&mut self, mut packet: Packet) {
        //TODO debug only
        println!(
            "{} [ Drone {} ]: has received the packet {:?}",
            "✓".green(),
            self.id,
            packet
        );

        //Step1
        //Step2

        if let PacketType::FloodRequest(flood_request) = packet.clone().pack_type {
            let flood_id = flood_request.flood_id;
            let flood_initiator = flood_request.initiator_id;
            self.handle_flood_request(flood_request, packet);
            self.flood_id_received.insert((flood_id, flood_initiator));
        } else if self.check_packet_correct_id(packet.clone()) {
            packet.routing_header.increase_hop_index();
            if packet.routing_header.hop_index == packet.routing_header.hops.len() {
                //Step3
                eprintln!(
                    "{} The selected destination in the RoutingHeader of [ Drone {} ] is a Drone",
                    "✗".red(),
                    self.id
                );
                self.send_nack(packet.clone(), None, NackType::DestinationIsDrone);
                return;
            }

            if !self.check_neighbor(packet.clone()) {
                //Step4
                let neighbor = packet.routing_header.hops[packet.routing_header.hop_index];
                eprintln!(
                    "{} [ Drone {} ]: can't send packet to Drone {} because it is not its neighbor",
                    "✗".red(),
                    self.id,
                    neighbor
                );
                self.send_nack(packet.clone(), None, NackType::ErrorInRouting(self.id));
                return;
            }

            println!(
                "{} Packet with [ session_id: {} ] is being handled from [ Drone {} ]",
                "✓".green(),
                packet.session_id,
                self.id
            );

            match packet.clone().pack_type {
                PacketType::Nack(_nack) => self.handle_ack_nack(packet),
                PacketType::Ack(_ack) => self.handle_ack_nack(packet),
                PacketType::MsgFragment(fragment) => self.handle_fragment(packet, fragment),
                PacketType::FloodRequest(_) => unreachable!(),
                PacketType::FloodResponse(flood_response) => {
                    self.handle_flood_response(flood_response, packet);
                }
            }
        }
    }

    //packet needs to be updated to the correct hop index
    fn send_message(&self, packet: Packet) -> bool {
        let destination = packet.routing_header.hops[packet.routing_header.hop_index];
        let packet_type = packet.pack_type.clone();
        if let Some(sender) = self.packet_send.get(&destination) {
            //create a nack and I send it to the previous node
            match sender.send(packet.clone()) {
                Ok(_) => {
                    println!(
                        "{} [ Drone {} ]: was sent a {} packet to [ Drone {} ]",
                        "✓".green(),
                        self.id,
                        packet_type,
                        destination
                    );
                    true
                }
                Err(e) => {
                    println!("{} [ Drone {} ]: Failed to send the {} to [ Drone {} ]: {}\n├─>{} Sending to Simulation Controller...",
                        "✗".red(),
                        self.id,
                        packet_type,
                        destination,
                        e,
                        "!!!".yellow()
                    );
                    //there is an error in sending the packet, the drone should send the packet to the simulation controller
                    self.controller_send
                        .send(DroneEvent::PacketDropped(packet))
                        .unwrap();
                    println!(
                        "└─>{} [ Drone {} ]: {} sent to Simulation Controller",
                        "!!!".yellow(),
                        self.id,
                        packet_type,
                    );

                    false
                }
            }
        } else {
            if let PacketType::MsgFragment(_) = packet_type {
                println!(
                    "{} [ Drone {} ]: does not exist in the path",
                    "✗".red(),
                    destination
                );
            } else {
                println!(
                    "{} [ Drone {} ]: Failed to send the {}: No connection to [ Drone {} ]\n├─>{} Sending to Simulation Controller...",
                    "✗".red(),
                    self.id,
                    packet_type,
                    destination,
                    "!!!".yellow()
                );
                self.controller_send
                    .send(DroneEvent::PacketDropped(packet))
                    .unwrap();
                println!(
                    "└─>{} [ Drone {} ]: {} sent to Simulation Controller",
                    "!!!".yellow(),
                    self.id,
                    packet_type,
                );
            }

            false
        }
    }

    fn check_packet_correct_id(&self, packet: Packet) -> bool {
        if self.id == packet.clone().routing_header.hops[packet.clone().routing_header.hop_index] {
            true
        } else {
            self.send_nack(packet, None, NackType::UnexpectedRecipient(self.id));
            println!(
                "{} [ Drone {} ]: does not correspond to the Drone indicated by the `hop_index`",
                "✗".red(),
                self.id
            );
            false
        }
    }

    fn handle_ack_nack(&mut self, mut packet: Packet) {
        if packet.routing_header.hop_index >= packet.routing_header.hops.len() {
            //it can't happen
            panic!("{} Source is not a client!", "PANIC".purple());
        }
        match packet.clone().pack_type {
            PacketType::Nack(nack) => {
                println!(
                    "{} [ Drone {} ]: received a {}\n├─>{} Checking [ Drone {} ] buffer...",
                    "!!!".yellow(),
                    self.id,
                    packet.pack_type,
                    "!!!".yellow(),
                    self.id
                );

                //check if it is in the buffer
                if let Some(fragment) = self
                    .buffer
                    .get_fragment(packet.clone().session_id, nack.fragment_index)
                {
                    println!(
                        "├─>{} Fragment [ fragment_index: {} ] of the Packet [ session_id: {} ]  was found in the buffer",
                        "✓".green(),
                        fragment.fragment_index,
                        packet.session_id
                    );

                    //re_send the fragment
                    //reverse the path
                    packet.routing_header.reverse();
                    let new_packet = Packet {
                        pack_type: PacketType::MsgFragment(fragment.clone()),
                        routing_header: packet.routing_header.clone(),
                        session_id: packet.session_id,
                    };
                    self.send_message(new_packet);

                    println!("└─>{} The Packet was sent", "✓".green());
                } else {
                    //send nack to the previous node
                    packet.routing_header.hop_index += 1; //to the previous
                    self.send_nack(packet, None, NackType::Dropped);
                }
            }
            _ => {
                if packet.routing_header.hop_index < packet.routing_header.hops.len() - 1 {
                    packet.routing_header.hop_index += 1; //to the previous
                } else {
                    println!(
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
    }

    fn handle_fragment(&mut self, packet: Packet, fragment: Fragment) {
        if self.check_drop_fragment() {
            println!(
                "{} Fragment [ fragment_index: {} ] of the Packet [ session_id: {} ] has been dropped by [ Drone {} ]",
                "!!!".yellow(),
                fragment.fragment_index,
                packet.session_id,
                self.id
            );
            self.send_nack(packet, Some(fragment), NackType::Dropped);
        } else {
            //add the fragment to the buffer
            println!(
                "{} [ Drone {} ]: forwarded the the fragment [ fragment_index: {} ] of the Packet [ session_id: {} ]",
                "✓".green(),
                self.id,
                fragment.fragment_index,
                packet.session_id
            );

            self.buffer
                .add_fragment(packet.clone().session_id, fragment);
            self.send_message(packet);

            println!(
                "└─>{} Fragment was added to the [ Drone {} ] buffer",
                "!!!".yellow(),
                self.id
            );
        }
    }

    //Useful to send nack to the previous node
    fn send_nack(&self, mut packet: Packet, fragment: Option<Fragment>, nack_type: NackType) {
        packet.routing_header.reverse();
        let prev_hop = packet.routing_header.next_hop().unwrap();
        if let Some(sender) = self.packet_send.get(&prev_hop) {
            let mut nack = Nack {
                fragment_index: 0, //if it isn't a fragment then i put the nack_type and index
                nack_type,
            };
            if let Some(frag) = fragment {
                //if it is a fragment then i set it
                nack = Nack {
                    fragment_index: frag.fragment_index,
                    nack_type: NackType::Dropped,
                };
            }
            packet.pack_type = PacketType::Nack(nack);

            match sender.send(packet.clone()) {
                Ok(_) => {
                    println!(
                        "{} Nack was sent from [ Drone {} ] to [ Drone {} ]",
                        "!!!".yellow(),
                        self.id,
                        prev_hop
                    );
                }
                Err(e) => {
                    println!("{} [ Drone {} ]: Failed to send the Nack to [ Drone {} ]: {}\n├─>{} Sending to Simulation Controller...",
                        "✗".red(),
                        self.id,
                        prev_hop,
                        e,
                        "!!!".yellow()
                    );
                    //there is an error in sending the packet, the drone should send the packet to the simulation controller
                    self.controller_send
                        .send(DroneEvent::PacketDropped(packet))
                        .unwrap();
                    println!(
                        "└─>{} [ Drone {} ]: sent A Nack to the Simulation Controller",
                        "!!!".yellow(),
                        self.id
                    );
                }
            }
        } else {
            //panic!("Drone before this {} does not exist", prev_hop);
            println!("{} [ Drone {} ]: Failed to send the Nack: No connection to {}\n├─>{} Sending to Simulation Controller...",
                "✗".red(),
                self.id,
                prev_hop,
                "!!!".yellow()
            );

            let mut nack = Nack {
                fragment_index: 0, //if it isn't a fragment then i put the nack_type and index
                nack_type,
            };
            if let Some(frag) = fragment {
                //if it is a fragment then i set it
                nack = Nack {
                    fragment_index: frag.fragment_index,
                    nack_type: NackType::Dropped,
                };
            }
            packet.pack_type = PacketType::Nack(nack);

            self.controller_send
                .send(DroneEvent::PacketDropped(packet))
                .unwrap();
            println!(
                "└─>{} [ Drone {} ]: sent A Nack to the Simulation Controller",
                "!!!".yellow(),
                self.id
            );
        }
    }

    fn check_neighbor(&self, packet: Packet) -> bool {
        let destination = packet.routing_header.hops[packet.routing_header.hop_index];
        self.packet_send.contains_key(&destination)
    }

    #[allow(clippy::cast_possible_truncation)]
    fn check_drop_fragment(&self) -> bool {
        let mut rng = rand::thread_rng();
        let val = rng.gen_range(1..=100);
        val <= (self.pdr * 100f32) as i32
    }

    fn handle_flood_request(&self, mut flood_request: FloodRequest, packet: Packet) {
        //neighbor that sent the packet
        let prev_node = if let Some(node) = flood_request.path_trace.last() {
            node.0
        } else {
            eprintln!("A drone can't be the first node in the path-trace.");
            todo!("how to tell controller we received a wrong path-trace")
        };

        //copied the path-trace in the flood_request and added the current node
        // let mut new_path_trace = flood_request.path_trace.clone();
        // new_path_trace.push((self.id, NodeType::Drone));
        flood_request.path_trace.push((self.id, NodeType::Drone));

        //let mut new_routing_header = packet.routing_header.clone();
        //new_routing_header.hop_index += 1;
        //new_routing_header.hops.push(self.id);

        //check if the flood_id has already been received
        if self
            .flood_id_received
            .contains(&(flood_request.flood_id, flood_request.initiator_id))
        {
            //sending new floodResponse to prev_node or to sim controller

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
            //we have not neighbor except the prev node
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
            //sending to every neighbors except previous one
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
                println!(
                    "{} [ Drone {} ]: sent a FloodRequest with flood_id: {} to the [ Drone {} ]",
                    "✓".green(),
                    self.id,
                    flood_request.flood_id,
                    neighbor.0
                );
            }
        }
    }

    fn handle_command(&mut self, command: DroneCommand) {
        match command {
            DroneCommand::AddSender(node_id, sender) => {
                if let std::collections::hash_map::Entry::Vacant(e) =
                    self.packet_send.entry(node_id)
                {
                    println!(
                        "{} Adding sender: {} to [ Drone {} ]",
                        "✓".green(),
                        node_id,
                        self.id,
                    );
                    e.insert(sender);
                } else {
                    println!(
                        "{} [ Drone {} ] is already connected to [ Drone {} ]",
                        "!!!".yellow(),
                        self.id,
                        node_id
                    );
                }
            }
            DroneCommand::SetPacketDropRate(pdr) => {
                if (0.0..=1.0).contains(&pdr) {
                    println!(
                        "{} Setting [ Drone {} ] pdr to: {}",
                        "✓".green(),
                        self.id,
                        pdr
                    );
                    self.pdr = pdr;
                } else {
                    println!(
                        "{} The pdr is a value that must be between `0.0` and `1.0`",
                        "✗".red()
                    );
                }
            }
            DroneCommand::RemoveSender(node_id) => {
                if !self.packet_send.contains_key(&node_id) {
                    println!(
                        "{} [ Drone {} ] is already disconnected from [ Drone {} ]",
                        "!!!".yellow(),
                        self.id,
                        node_id
                    );
                } else {
                    println!(
                        "{} Removing sender: {} from [ Drone {} ]",
                        "✓".green(),
                        node_id,
                        self.id
                    );
                    self.packet_send.remove(&node_id);
                }
            }
            DroneCommand::Crash => unreachable!(),
        }
    }

    fn handle_flood_response(&self, flood_response: FloodResponse, packet: Packet) {
        let new_routing_header = packet.routing_header.clone();

        //new_routing_header.hop_index -= 1;

        let new_packet = Packet {
            pack_type: PacketType::FloodResponse(flood_response.clone()),
            routing_header: new_routing_header.clone(),
            session_id: packet.session_id,
        };

        if let Some(sender) = self
            .packet_send
            .get(&new_routing_header.hops[new_routing_header.hop_index])
        {
            match sender.send(new_packet.clone()) {
                Ok(()) => println!(
                    "{} [ Drone {} ]: sent a FloodResponse with flood_id: {} to [ Drone {} ]",
                    "✓".green(),
                    self.id,
                    flood_response.flood_id,
                    &new_routing_header.hops[new_routing_header.hop_index]
                ),
                Err(e) => {
                    println!(
                        "{} [ Drone {} ]: failed to send the FloodResponse to the Drone {}: {}\n├─>{} Sending to Simulation Controller...",
                        "✗".red(),
                        self.id,
                        &new_routing_header.hops[new_routing_header.hop_index],
                        e,
                        "!!!".yellow()
                    );
                    self.controller_send
                        .send(DroneEvent::PacketDropped(new_packet))
                        .unwrap();
                    println!(
                        "└─>{} [ Drone {} ]: sent the FloodResponse to the Simulation Controller",
                        "!!!".yellow(),
                        self.id
                    );
                }
            }
        } else {
            println!(
                "{} [ Drone {} ]: failed to send the FloodResponse: No connection to [ Drone {} ]\n├─>{} Sending to Simulation Controller...",
                "✗".red(),
                self.id,
                &new_routing_header.hops[new_routing_header.hop_index],
                "!!!".yellow()
            );

            // println!("[ Drone {} ]: {:?}", self.id, flood_response);

            self.controller_send
                .send(DroneEvent::PacketDropped(new_packet))
                .unwrap();
            println!(
                "└─>{} [ Drone {} ]: sent the FloodResponse to the Simulation Controller",
                "!!!".yellow(),
                self.id
            );
        }
    }

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

        match dest_node.1.send(new_packet.clone()) {
            Ok(()) => println!(
                "{} [ Drone {} ]: sent the FloodRequest with flood_id: {} sent to [ Drone {} ]",
                "✓".green(),
                self.id,
                flood_id,
                dest_node.0
            ),
            Err(e) => println!(
                "{} [ Drone {} ]: failed to send FloodRequest to the [ Drone {} ]: {}",
                "✗".red(),
                self.id,
                dest_node.0,
                e
            ),
        };
    }

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

        //decreasing hop_index for default
        //routing_header.hop_index -= 1;

        let new_packet = Packet {
            pack_type: PacketType::FloodResponse(flood_response.clone()),
            routing_header,
            session_id,
        };

        if let Some(sender) = self.packet_send.get(&dest_node) {
            match sender.send(new_packet.clone()) {
                Ok(()) => println!(
                    "{} [ Drone {} ]: sent the FloodResponse to [ Drone {} ]\n└─>Reason: {}",
                    "✓".green(),
                    self.id,
                    dest_node,
                    reason
                ),
                Err(e) => {
                    println!(
                        "{} [ Drone {} ]: Failed to send the FloodResponse to [ Drone {} ]: {}\n├─>{} Sending to Simulation Controller...",
                        "✗".red(),
                        self.id,
                        dest_node,
                        e,
                        "!!!".yellow()
                    );
                    //there is an error in sending the packet, the drone should send the packet to the simulation controller
                    self.controller_send
                        .send(DroneEvent::PacketDropped(new_packet))
                        .unwrap();
                    println!(
                        "└─>{} [ Drone {} ]: FloodResponse sent to Simulation Controller",
                        "!!!".yellow(),
                        self.id
                    );
                }
            }
        } else {
            //there is an error in sending the packet, the drone should send the packet to the simulation controller
            println!(
                "{} [ Drone {} ]: Failed to send the FloodResponse: No connection to [ Drone {} ]\n├─>{} Sending to Simulation Controller...",
                "✗".red(),
                self.id,
                dest_node,
                "!!!".yellow()
            );

            self.controller_send
                .send(DroneEvent::PacketDropped(new_packet))
                .unwrap();
            println!(
                "└─>{} [ Drone {} ]: FloodResponse sent to Simulation Controller",
                "!!!".yellow(),
                self.id
            );
        }
    }
}
