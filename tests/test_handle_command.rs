use rustastic_drone::RustasticDrone;

use crossbeam_channel::unbounded;
use std::{collections::HashMap, thread, time::Duration};
use wg_2024::{controller::DroneCommand, drone::Drone};

#[test]
fn test_add_sender() {
    let (send_to_drone, receive_from_drone) = unbounded();

    let mut drone = RustasticDrone::new(
        0,
        unbounded().0,
        receive_from_drone,
        unbounded().1,
        HashMap::new(),
        0f32,
    );
    let join = thread::spawn(move || drone.run());

    send_to_drone
        .send(DroneCommand::AddSender(1, unbounded().0))
        .unwrap();

    thread::sleep(Duration::from_secs(2));
    send_to_drone.send(DroneCommand::Crash).unwrap();
    join.join().unwrap();
}
#[test]
fn test_set_pdr() {
    let (send_to_drone, receive_from_drone) = unbounded();

    let mut drone = RustasticDrone::new(
        0,
        unbounded().0,
        receive_from_drone,
        unbounded().1,
        HashMap::new(),
        0f32,
    );
    let join = thread::spawn(move || drone.run());

    send_to_drone
        .send(DroneCommand::SetPacketDropRate(0.05))
        .unwrap();

    thread::sleep(Duration::from_secs(2));
    send_to_drone.send(DroneCommand::Crash).unwrap();
    join.join().unwrap();
}
#[test]
fn test_remove_sender() {
    let (send_to_drone, receive_from_drone) = unbounded();

    let mut drone = RustasticDrone::new(
        0,
        unbounded().0,
        receive_from_drone,
        unbounded().1,
        HashMap::new(),
        0f32,
    );
    let join = thread::spawn(move || drone.run());

    send_to_drone
        .send(DroneCommand::AddSender(2, unbounded().0))
        .unwrap();
    send_to_drone.send(DroneCommand::RemoveSender(2)).unwrap();

    thread::sleep(Duration::from_secs(2));
    send_to_drone.send(DroneCommand::Crash).unwrap();
    join.join().unwrap();
}
