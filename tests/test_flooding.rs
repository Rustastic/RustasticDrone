use crossbeam_channel::{select_biased, unbounded, Receiver, Sender};
use std::{
    collections::{HashMap, HashSet},
    thread,
    time::Duration,
    vec,
};
use wg_2024::{
    controller::{DroneCommand, DroneEvent},
    drone::Drone,
    network::{NodeId, SourceRoutingHeader},
    packet::{FloodRequest, FloodResponse, NodeType, Packet, PacketType},
};

use drone::RustasticDrone;
use simulation_controller::SimulationController;

struct Host {
    id: NodeId,
    controller_send: Sender<DroneEvent>,
    controller_recv: Receiver<DroneCommand>,
    packet_recv: Receiver<Packet>,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    flood_id_received: HashSet<(u64, NodeId)>,
}

impl Host {
    fn new(
        id: NodeId,
        controller_send: Sender<DroneEvent>,
        controller_recv: Receiver<DroneCommand>,
        packet_recv: Receiver<Packet>,
        packet_send: HashMap<NodeId, Sender<Packet>>,
    ) -> Self {
        Self {
            id,
            packet_recv,
            packet_send,
            controller_recv,
            controller_send,
            flood_id_received: HashSet::new(),
        }
    }

    fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.controller_recv) -> command => {
                    if let Ok(command) = command {
                        match command {
                            DroneCommand::Crash => {
                                println!("Host {} crashed", self.id);
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

    fn handle_packet(&mut self, mut packet: Packet) {
        match packet.clone().pack_type {
            PacketType::FloodRequest(flood_request) => {
                let flood_id = flood_request.flood_id;
                let flood_initiator = flood_request.initiator_id;
                self.handle_flood_request(flood_request, packet);
                self.flood_id_received.insert((flood_id, flood_initiator));
            }
            PacketType::FloodResponse(flood_response) => {
                if flood_response.flood_id == self.id as u64 {
                    self.process_flood_response(flood_response);
                } else {
                    packet.routing_header.hop_index += 1;
                    self.handle_flood_response(flood_response, packet);
                }
            }
            _ => unimplemented!(),
        }
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
        flood_request.path_trace.push((self.id, NodeType::Client));

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
                    "a FloodRequest with flood_id: {} was already received",
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
                    "There are no other neighbors to send the FloodRequest with flood_id: {}",
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
            }
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
                    "[{}]: FloodResponse sent to {}\tflood_id: {}",
                    self.id,
                    &new_routing_header.hops[new_routing_header.hop_index],
                    flood_response.flood_id
                ),
                Err(e) => {
                    println!("[{}]: Failed to send the FloodResponse to {}: {e}\tSending to Simulation Controller...",
                    self.id, &new_routing_header.hops[new_routing_header.hop_index]);
                    self.controller_send
                        .send(DroneEvent::PacketDropped(new_packet))
                        .unwrap();
                    println!("[{}]: FloodResponse sent to Simulation Controller", self.id);
                }
            }
        } else {
            println!(
        "[{}]: Failed to send the FloodResponse: No connection to {}\tSending to Simulation Controller...",
        self.id, &new_routing_header.hops[new_routing_header.hop_index]
    );

            println!("[{}]: {:?}", self.id, flood_response);

            self.controller_send
                .send(DroneEvent::PacketDropped(new_packet))
                .unwrap();
            println!("[{}]: FloodResponse sent to Simulation Controller", self.id);
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
                "[{}]: FloodRequest sent to {}\tflood_id: {}",
                self.id, dest_node.0, flood_id
            ),
            Err(e) => println!(
                "[{}]: Failed to send FloodRequest to {}: {e}",
                self.id, dest_node.0
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
                    "[{}]: FloodResponse sent to {dest_node}\tReason: {reason}",
                    self.id
                ),
                Err(e) => {
                    println!("[{}]: Failed to send the FloodResponse to {dest_node}: {e}\tSending to Simulation Controller...", self.id);
                    //there is an error in sending the packet, the drone should send the packet to the simulation controller
                    self.controller_send
                        .send(DroneEvent::PacketDropped(new_packet))
                        .unwrap();
                    println!("[{}]: FloodResponse sent to Simulation Controller", self.id);
                }
            }
        } else {
            //there is an error in sending the packet, the drone should send the packet to the simulation controller
            println!("[{}]: Failed to send the FloodResponse: No connection to {dest_node}\tSending to Simulation Controller...", self.id);
            println!("[{}]: {:?}", self.id, flood_response);
            self.controller_send
                .send(DroneEvent::PacketDropped(new_packet))
                .unwrap();
            println!("[{}]: FloodResponse sent to Simulation Controller", self.id);
        }
    }

    fn process_flood_response(&self, flood_response: FloodResponse) {
        println!("{:?}", flood_response);
    }

    fn start_flooding(&self) {
        let flood_request = FloodRequest {
            flood_id: self.id as u64,
            initiator_id: self.id,
            path_trace: vec![(self.id, NodeType::Client)],
        };

        let routing_header = SourceRoutingHeader {
            hops: vec![],
            hop_index: 0,
        };

        let session_id = 0;

        for neighbor in self.packet_send.iter() {
            self.send_flood_request(neighbor, &flood_request, routing_header.clone(), session_id);
        }
    }

    fn handle_command(&mut self, command: DroneCommand) {
        match command {
            DroneCommand::AddSender(node_id, sender) => {
                println!("Adding sender: {node_id}");
                self.packet_send.insert(node_id, sender);
            }
            DroneCommand::SetPacketDropRate(_pdr) => {
                println!("starting a new flood!");
                self.start_flooding();
            }
            DroneCommand::RemoveSender(node_id) => {
                println!("Removing sender: {node_id}");
                self.packet_send.remove(&node_id);
            }
            DroneCommand::Crash => unreachable!(),
        }
    }
}

#[test]
fn test_flooding() {
    let mut channels: HashMap<u8, (Sender<Packet>, Receiver<Packet>)> = HashMap::new();
    let connections: Vec<Vec<u8>> = vec![
        vec![1, 2, 6],
        vec![0, 2, 3, 7],
        vec![0, 1, 3, 4, 5],
        vec![1, 2, 4, 7],
        vec![2, 3, 8],
        vec![2, 8],
        vec![0],
        vec![1, 3],
        vec![4, 5],
    ];
    let pdrs: Vec<f32> = vec![0.0, 0.1, 0.2, 0.1, 0.3, 0.4];

    for i in 0u8..9u8 {
        channels.insert(i, unbounded());
    }

    let mut controller_drones = HashMap::new();
    let (node_event_send, node_event_recv) = unbounded();

    let mut handles = Vec::new();

    for i in 0u8..6u8 {
        let (controller_drone_send, controller_drone_recv) = unbounded();
        controller_drones.insert(i, controller_drone_send);
        let node_event_send = node_event_send.clone();
        // packet
        let packet_recv = channels[&i].1.clone();
        let packet_send = connections[i as usize]
            .clone()
            .into_iter()
            .map(|id| (id, channels[&id].0.clone()))
            .collect();

        let pdr = pdrs[i as usize];

        handles.push(thread::spawn(move || {
            let mut drone = RustasticDrone::new(
                i,
                node_event_send,
                controller_drone_recv,
                packet_recv,
                packet_send,
                pdr,
            );

            drone.run();
        }));
    }

    for i in 6u8..9u8 {
        let (controller_drone_send, controller_drone_recv) = unbounded();
        controller_drones.insert(i, controller_drone_send);
        let node_event_send = node_event_send.clone();
        // packet
        let packet_recv = channels[&i].1.clone();
        let packet_send = connections[i as usize]
            .clone()
            .into_iter()
            .map(|id| (id, channels[&id].0.clone()))
            .collect();
        handles.push(thread::spawn(move || {
            let mut host = Host::new(
                i,
                node_event_send,
                controller_drone_recv,
                packet_recv,
                packet_send,
            );

            host.run();
        }));
    }

    controller_drones
        .get(&7)
        .unwrap()
        .send(DroneCommand::SetPacketDropRate(0.0))
        .unwrap();
    thread::sleep(Duration::from_secs(5));
    let mut controller = SimulationController::new(controller_drones, node_event_recv);
    controller.crash_all();

    while let Some(handle) = handles.pop() {
        handle.join().unwrap();
    }
}
