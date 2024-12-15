use drone::RustasticDrone;

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

/*#[test]
fn test_handle_ack_nack() {
    // Set up three drones
    let (drone1, controller_send1, packet_send1, event_recv1, _neighbor_recv2) = setup_drone();
    let (drone2, _controller_send2, packet_send2, _event_recv2, _neighbor_recv3) =
        setup_drone();
    let (drone3, _controller_send3, packet_send3, _event_recv3, _neighbor_recv4) =
        setup_drone();

    // Connect drones' packet channels
    drone1
        .lock()
        .unwrap()
        .packet_send
        .insert(2, packet_send2.clone());
    drone2
        .lock()
        .unwrap()
        .packet_send
        .insert(1, packet_send1.clone());
    drone2
        .lock()
        .unwrap()
        .packet_send
        .insert(3, packet_send3.clone());
    drone3
        .lock()
        .unwrap()
        .packet_send
        .insert(2, packet_send2.clone());

    // Create a routing header and a packet fragment
    let routing_header = SourceRoutingHeader {
        hops: vec![1, 2, 3],
        hop_index: 1,
    };
    let data = [1; FRAGMENT_DSIZE]; // Example data
    let fragment = Fragment {
        fragment_index: 0,
        total_n_fragments: 1,
        length: data.len() as u8,
        data,
    };
    let packet = Packet {
        pack_type: PacketType::MsgFragment(fragment.clone()),
        routing_header: routing_header.clone(),
        session_id: 42,
    };

    // Send the packet from drone1 to drone2
    packet_send1.send(packet.clone()).unwrap();

    // Process packet on drone2 and simulate failure, generating a NACK
    let handle2 = thread::spawn(move || {
        let mut drone2 = drone2.lock().unwrap();
        drone2.run();
    });
    thread::sleep(Duration::from_millis(100)); // Allow some processing time

    // Simulate a NACK on drone2 for a dropped packet
    let nack = Nack {
        fragment_index: fragment.fragment_index,
        nack_type: NackType::Dropped,
    };
    let nack_packet = Packet {
        pack_type: PacketType::Nack(nack.clone()),
        routing_header: routing_header.clone(),
        session_id: 42,
    };
    packet_send2.send(nack_packet.clone()).unwrap();

    // Run drone1 to handle the NACK
    let handle1 = thread::spawn(move || {
        let mut drone1 = drone1.lock().unwrap();
        drone1.run();
    });
    thread::sleep(Duration::from_millis(100)); // Allow some processing time

    // Verify that drone1 received the NACK
    if let Ok(event) = event_recv1.try_recv() {
        if let DroneEvent::PacketDropped(received_packet) = event {
            if let PacketType::Nack(received_nack) = received_packet.pack_type {
                assert_eq!(received_nack.fragment_index, nack.fragment_index);
                assert!(matches!(received_nack.nack_type, NackType::Dropped));
                println!("Drone1 successfully received the NACK");
            } else {
                panic!("Expected a NACK packet, but received a different type");
            }
        } else {
            panic!("Unexpected event received: {:?}", event);
        }
    } else {
        panic!("Drone1 did not receive any events");
    }

    handle2.join().unwrap();
    handle1.join().unwrap();
}

#[test]
fn test_check_neighbor() {
    // Setup the test drone and its communication channels
    let (drone, _controller_send, packet_send, _event_recv, _neighbor_recv) = setup_drone();

    // Add a neighbor that exists
    let existing_node_id = 2;
    let (sender, _receiver) = unbounded();
    drone
        .lock()
        .unwrap()
        .packet_send
        .insert(existing_node_id, sender);

    // Create a packet to test with
    let routing_header = SourceRoutingHeader {
        hop_index: 1,     // Initial hop index
        hops: vec![1, 2], // Drone ID and its neighbor
    };
    let packet = Packet {
        routing_header,
        session_id: 12345,
        pack_type: PacketType::Ack(Ack { fragment_index: 1 }), // Example packet type
    };

    // Test for an existing neighbor
    assert!(drone.lock().unwrap().check_neighbor(packet.clone())); // Should return true

    // Create a packet with a non-existing neighbor
    let routing_header_non_existent = SourceRoutingHeader {
        hop_index: 1,
        hops: vec![1, 3], // Drone ID and a non-existing neighbor
    };
    let packet_non_existent = Packet {
        routing_header: routing_header_non_existent,
        session_id: 12346,
        pack_type: PacketType::Ack(Ack { fragment_index: 1 }), // Example packet type
    };

    // Test for a non-existing neighbor
    assert!(!drone.lock().unwrap().check_neighbor(packet_non_existent)); // Should return false
}*/

/*#[test]
fn test_send_nack() {
    let (drone1, controller_send1, packet_send1, event_recv1, _neighbor_recv2) = setup_drone();
    let (drone2, _controller_send2, packet_send2, _event_recv2, neighbor_recv2) = setup_drone();

    // Clone the Arc<Mutex<T>> for passing into a new thread
    let drone2_clone = Arc::clone(&drone2);

    // Set up and run drone2 in its own thread
    let handle = thread::spawn(move || {
        let mut drone2 = drone2_clone.lock().unwrap();
        drone2.run(); // Run drone2 inside the lock
    });

    // Allow some time for processing
    thread::sleep(Duration::from_millis(100));

    // Scenario: Sending a Nack for ErrorInRouting
    let nack_type_error = NackType::ErrorInRouting(3); // Targeting a non-neighboring drone
    let packet_error = Packet {
        routing_header: SourceRoutingHeader {
            hop_index: 1,
            hops: vec![1, 3], // Indicates that the next hop is not a neighbor
        },
        session_id: 12345,
        pack_type: PacketType::Nack(Nack {
            nack_type: nack_type_error,
            fragment_index: 0,
        }),
    };

    drone1
        .lock()
        .unwrap()
        .send_nack(packet_error.clone(), None, NackType::ErrorInRouting(3));

    // Check drone2 does not receive this Nack with a timeout
    if let Ok(received_packet) = drone2
        .lock()
        .unwrap()
        .packet_recv
        .recv_timeout(Duration::from_millis(500))
    {
        panic!(
            "Unexpected packet received by drone2: {:?}",
            received_packet
        );
    } else {
        println!("Drone2 correctly did not receive the NACK.");
    }

    // Scenario: Sending a Nack with DestinationIsDrone
    let nack_type_dest_drone = NackType::DestinationIsDrone;
    let packet_dest_drone = Packet {
        routing_header: SourceRoutingHeader {
            hop_index: 1,
            hops: vec![1, 1], // Route to itself, indicating a drone is the destination
        },
        session_id: 12346,
        pack_type: PacketType::Nack(Nack {
            nack_type: nack_type_dest_drone,
            fragment_index: 0,
        }),
    };

    drone1.lock().unwrap().send_nack(
        packet_dest_drone.clone(),
        None,
        NackType::DestinationIsDrone,
    );

    if let Ok(received_packet) = drone2
        .lock()
        .unwrap()
        .packet_recv
        .recv_timeout(Duration::from_millis(500))
    {
        if let PacketType::Nack(nack) = received_packet.pack_type {
            assert!(nack.nack_type == NackType::DestinationIsDrone);
        } else {
            panic!("Expected a NACK packet, but received a different type.");
        }
    } else {
        panic!("Drone2 did not receive any packet within the timeout period.");
    }

    // Scenario: Sending a Nack with UnexpectedRecipient
    let nack_type_unexpected = NackType::UnexpectedRecipient(2);
    let packet_unexpected = Packet {
        routing_header: SourceRoutingHeader {
            hop_index: 1,
            hops: vec![1, 2], // Next hop is a drone not expected to receive this
        },
        session_id: 12347,
        pack_type: PacketType::Nack(Nack {
            nack_type: nack_type_unexpected,
            fragment_index: 0,
        }),
    };

    drone1.lock().unwrap().send_nack(
        packet_unexpected.clone(),
        None,
        NackType::UnexpectedRecipient(2),
    );

    if let Ok(received_packet) = drone2
        .lock()
        .unwrap()
        .packet_recv
        .recv_timeout(Duration::from_millis(500))
    {
        if let PacketType::Nack(nack) = received_packet.pack_type {
            assert!(nack.nack_type == NackType::UnexpectedRecipient(2));
        } else {
            panic!("Expected a NACK packet, but received a different type.");
        }
    } else {
        panic!("Drone2 did not receive any packet within the timeout period.");
    }

    handle.join().unwrap();
}*/
