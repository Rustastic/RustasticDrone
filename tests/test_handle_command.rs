use rustastic_drone::RustasticDrone;

use crossbeam_channel::unbounded;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use wg_2024::{controller::DroneCommand, drone::Drone};

#[test]
fn test_add_sender() {
    let (drone_to_controller, _controller_from_drone) = unbounded();
    let (controller_to_drone, drone_from_controller) = unbounded();

    let drone_thread = Arc::new(Mutex::new(RustasticDrone::new(
        1,
        drone_to_controller.clone(),
        drone_from_controller,
        unbounded().1,
        HashMap::new(),
        0f32,
    )));
    println!("{drone_thread:?}");
    let drone = drone_thread.clone();

    let handler = thread::spawn(move || drone_thread.lock().unwrap().run());

    controller_to_drone
        .send(DroneCommand::AddSender(15, unbounded().0))
        .unwrap();
    thread::sleep(Duration::from_secs(1));

    controller_to_drone.send(DroneCommand::Crash).unwrap();
    handler.join().unwrap();
    let drone = drone.lock().unwrap();
    println!("{drone:?}");
    //TODO cannot access private field because test is not a submodule of drone
    // assert_ne!(drone.packet_send.iter().last().0, 15);
}

#[test]
fn test_set_pdr() {
    let (drone_to_controller, _controller_from_drone) = unbounded();
    let (controller_to_drone, drone_from_controller) = unbounded();

    let drone_thread = Arc::new(Mutex::new(RustasticDrone::new(
        1,
        drone_to_controller.clone(),
        drone_from_controller,
        unbounded().1,
        HashMap::new(),
        0f32,
    )));
    println!("{drone_thread:?}");
    let drone = drone_thread.clone();

    let handler = thread::spawn(move || drone_thread.lock().unwrap().run());

    controller_to_drone
        .send(DroneCommand::SetPacketDropRate(0.05))
        .unwrap();

    thread::sleep(Duration::from_secs(1));
    controller_to_drone.send(DroneCommand::Crash).unwrap();
    handler.join().unwrap();

    let drone = drone.lock().unwrap();
    println!("{drone:?}");
    //TODO cannot access private field because test is not a submodule of drone
    // assert_ne!(drone.pdr, 0.05);
}
#[test]
fn test_remove_sender() {
    let (drone_to_controller, _controller_from_drone) = unbounded();
    let (controller_to_drone, drone_from_controller) = unbounded();
    let mut sender = HashMap::new();
    sender.insert(2, unbounded().0);
    let drone_thread = Arc::new(Mutex::new(RustasticDrone::new(
        1,
        drone_to_controller.clone(),
        drone_from_controller,
        unbounded().1,
        sender,
        0f32,
    )));
    println!("{drone_thread:?}");
    let drone = drone_thread.clone();

    let handler = thread::spawn(move || drone_thread.lock().unwrap().run());
    controller_to_drone
        .send(DroneCommand::RemoveSender(2))
        .unwrap();

    thread::sleep(Duration::from_secs(1));
    controller_to_drone.send(DroneCommand::Crash).unwrap();
    handler.join().unwrap();

    let drone = drone.lock().unwrap();
    println!("{drone:?}");
    //TODO cannot access private field because test is not a submodule of drone
    // assert_eq!( drone.packet_send.iter().last().1, 2 ) ;
}
