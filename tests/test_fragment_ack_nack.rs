use rustastic_drone::RustasticDrone;

use crossbeam_channel::{unbounded, Receiver, Sender};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use wg_2024::{
    controller::{DroneCommand, DroneEvent},
    drone::Drone,
    network::SourceRoutingHeader,
    packet::{Ack, Fragment, Nack, NackType, Packet, PacketType},
};

const FRAGMENT_DSIZE: usize = 128;

/// Setup per creare un drone
fn setup_drone() -> (
    Arc<Mutex<RustasticDrone>>,
    Sender<DroneCommand>,
    Sender<Packet>,
    Receiver<DroneEvent>,
    Receiver<Packet>, // Per ricevere pacchetti simulati
) {
    let (controller_send, controller_recv) = unbounded();
    let (packet_send, packet_recv) = unbounded();
    let (event_send, event_recv) = unbounded();
    let (neighbor_send_1, _neighbor_recv_1) = unbounded(); // Nodo 1
    let (neighbor_send_2, neighbor_recv_2) = unbounded(); // Nodo 2

    let mut packet_send_map = HashMap::new();
    packet_send_map.insert(1, neighbor_send_1); // Nodo 1
    packet_send_map.insert(2, neighbor_send_2); // Nodo 2

    let drone = RustasticDrone::new(
        1, // Drone ID
        event_send,
        controller_recv,
        packet_recv,
        packet_send_map,
        0.0, // Nessun packet drop rate per test prevedibili
    );

    (
        Arc::new(Mutex::new(drone)),
        controller_send,
        packet_send,
        event_recv,
        neighbor_recv_2, // Nodo 2 è il vicino testato
    )
}

///BAD TEST
#[test]
fn test_send_fragment() {
    // Setup
    let (drone, controller_send, packet_send, _event_recv, neighbor_recv) = setup_drone();
    let drone_clone = Arc::clone(&drone);

    // Avvia il drone in un thread separato
    let join = thread::spawn(move || {
        let mut drone = drone_clone.lock().unwrap();
        drone.run();
    });

    // Crea un frammento
    let data = [1; FRAGMENT_DSIZE];
    let fragment = Fragment {
        fragment_index: 0,
        total_n_fragments: 1,
        length: data.len() as u8,
        data,
    };

    // Crea un pacchetto contenente il frammento
    let packet = Packet {
        pack_type: PacketType::MsgFragment(fragment.clone()),
        routing_header: SourceRoutingHeader {
            hops: vec![1, 2],
            hop_index: 0,
        },
        session_id: 123,
    };

    // Invia il pacchetto al drone
    packet_send.send(packet).unwrap();

    // Attendi che il pacchetto venga elaborato
    thread::sleep(Duration::from_secs(1));

    // Verifica che il vicino abbia ricevuto il frammento
    let received_packet = neighbor_recv.try_recv();
    assert!(
        received_packet.is_ok(),
        "Neighbor did not receive the fragment as expected."
    );

    // Verifica che il pacchetto ricevuto sia un MsgFragment
    if let PacketType::MsgFragment(received_fragment) = received_packet.unwrap().pack_type {
        assert_eq!(
            received_fragment.fragment_index, 0,
            "Fragment index mismatch."
        );
    } else {
        panic!("Expected MsgFragment, but got something else.");
    }

    // Termina il drone
    controller_send.send(DroneCommand::Crash).unwrap();
    join.join().unwrap();
}

///BAD TEST
#[test]
fn test_handle_nack() {
    let (drone, controller_send, packet_send, _event_recv, neighbor_recv) = setup_drone();
    let drone_clone = Arc::clone(&drone);

    let join = thread::spawn(move || {
        let mut drone = drone_clone.lock().unwrap();
        drone.run();
    });

    // Aggiungi un frammento al buffer
    let data = [1; FRAGMENT_DSIZE];
    let fragment = Fragment {
        fragment_index: 0,
        total_n_fragments: 1,
        length: data.len() as u8,
        data,
    };

    {
        let mut drone = drone.lock().unwrap();
        drone.buffer.add_fragment(123, fragment.clone());
    }

    // Crea un pacchetto NACK
    let nack = Packet {
        pack_type: PacketType::Nack(Nack {
            nack_type: NackType::Dropped,
            fragment_index: 0,
        }),
        routing_header: SourceRoutingHeader {
            hops: vec![1, 2], // Nodo 1 è il precedente
            hop_index: 0,
        },
        session_id: 123,
    };

    // Invia il NACK al drone
    packet_send.send(nack).unwrap();

    // Attendi che il pacchetto venga elaborato
    thread::sleep(Duration::from_secs(1));

    // Verifica che il vicino abbia ricevuto il frammento ritrasmesso
    let received_packet = neighbor_recv.try_recv();
    assert!(
        received_packet.is_ok(),
        "Neighbor did not receive the retransmitted fragment."
    );

    // Verifica che il pacchetto ricevuto sia un MsgFragment
    if let PacketType::MsgFragment(received_fragment) = received_packet.unwrap().pack_type {
        assert_eq!(
            received_fragment.fragment_index, 0,
            "Fragment index mismatch in retransmission."
        );
    } else {
        panic!("Expected MsgFragment, but got something else.");
    }

    // Termina il drone
    controller_send.send(DroneCommand::Crash).unwrap();
    join.join().unwrap();
}

///BAD TEST
#[test]
fn test_handle_ack() {
    let (drone, controller_send, packet_send, _event_recv, neighbor_recv) = setup_drone();
    let drone_clone = Arc::clone(&drone);

    let join = thread::spawn(move || {
        let mut drone = drone_clone.lock().unwrap();
        drone.run();
    });

    // Crea un pacchetto ACK
    let ack = Packet {
        pack_type: PacketType::Ack(Ack { fragment_index: 0 }),
        routing_header: SourceRoutingHeader {
            hops: vec![1, 2], // Nodo 1 è il precedente
            hop_index: 1,     // Il drone sta processando il nodo 1
        },
        session_id: 123,
    };

    // Invia l'ACK al drone
    packet_send.send(ack).unwrap();

    // Attendi che il pacchetto venga elaborato
    thread::sleep(Duration::from_secs(1));

    // Verifica che il vicino non abbia ricevuto ritrasmissioni
    let received_packet = neighbor_recv.try_recv();
    assert!(
        received_packet.is_err(),
        "Neighbor received an unexpected retransmission."
    );

    // Termina il drone
    controller_send.send(DroneCommand::Crash).unwrap();
    join.join().unwrap();
}