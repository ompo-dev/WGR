//! Contrato offline de publicação do input e escolha inicial em memory64.

use std::sync::atomic::Ordering;
use wgr_player::{
    engine,
    input::{BLOCK_BYTES, FOCUS, InputBlock, MX, MY},
    loader::{Launch, StartMode, engine_args},
};

const FIXTURE: &str = r#"
(module
  (import "env" "memory" (memory i64 2 2 shared))
  (import "env" "wasm_input_publish_js" (func $publish (param i64)))
  (func (export "publish") (param i64) local.get 0 call $publish)
)
"#;

#[test]
fn start_mode_is_not_an_engine_argument() {
    for mode in [
        StartMode::Menu,
        StartMode::Story,
        StartMode::Sandbox,
        StartMode::SandboxEnvTest,
    ] {
        let mut launch = Launch::new(1280, 720);
        launch.start_mode = mode;
        assert!(
            !engine_args(&launch)
                .iter()
                .any(|arg| arg.starts_with("-wasmstartchoice=")),
            "a escolha {mode:?} pertence ao bloco de input"
        );
    }
}

#[test]
fn initializes_the_block_before_publishing_and_updates_the_pending_choice() {
    let mut host = engine::load(&wat::parse_str(FIXTURE).unwrap(), 2, 131072).unwrap();
    let mut launch = Launch::new(1920, 1200);
    launch.start_mode = StartMode::Story;
    host.configure_input(&launch).unwrap();
    host.host.set_start_mode(StartMode::SandboxEnvTest).unwrap();
    let block = InputBlock::new(host.memory.clone(), 1024).unwrap();
    let publish = host
        .instance
        .get_typed_func::<i64, ()>(&mut host.store, "publish")
        .unwrap();
    publish.call(&mut host.store, 1024).unwrap();
    assert_eq!(host.host.input_block.load(Ordering::Acquire), 1024);
    assert_eq!(block.size().unwrap(), (1920, 1200));
    assert_eq!(block.load_word(MX).unwrap(), 960);
    assert_eq!(block.load_word(MY).unwrap(), 600);
    assert_eq!(block.load_word(FOCUS).unwrap(), 0);
    assert_eq!(block.start_mode().unwrap(), 3);
    for (mode, expected) in [
        (StartMode::Menu, 0),
        (StartMode::Story, 1),
        (StartMode::Sandbox, 2),
    ] {
        host.host.set_start_mode(mode).unwrap();
        assert_eq!(block.start_mode().unwrap(), expected);
    }
    block.set_key(0x57, true).unwrap();
    block.set_absolute(5, 7).unwrap();
    publish.call(&mut host.store, 1024).unwrap();
    assert_eq!(block.key(0x57).unwrap(), 0x80);
    assert_eq!(block.load_word(MX).unwrap(), 5);
    assert_eq!(block.load_word(MY).unwrap(), 7);
    assert_eq!(block.start_mode().unwrap(), 2);
    assert!(publish.call(&mut host.store, 2048).is_err());
    assert_eq!(host.host.input_block.load(Ordering::Acquire), 1024);
    assert!(host.configure_input(&launch).is_err());
}

#[test]
fn validates_geometry_and_mode_without_changing_the_block() {
    let mut host = engine::load(&wat::parse_str(FIXTURE).unwrap(), 2, 131072).unwrap();
    for (width, height) in [(0, 720), (1280, 0), (u32::MAX, 720), (1280, u32::MAX)] {
        assert!(host.configure_input(&Launch::new(width, height)).is_err());
    }
    let publish = host
        .instance
        .get_typed_func::<i64, ()>(&mut host.store, "publish")
        .unwrap();
    let address = 131072 - BLOCK_BYTES as i64;
    publish.call(&mut host.store, address).unwrap();
    let block = InputBlock::new(host.memory.clone(), address as u64).unwrap();
    assert_eq!(
        block.start_mode().unwrap(),
        0,
        "host sem launch continua esperando uma escolha"
    );
    for mode in [-1, 4, i32::MAX] {
        assert!(block.set_start_mode(mode).is_err());
        assert_eq!(block.start_mode().unwrap(), 0);
    }
}

#[test]
fn start_with_launch_configures_input_before_the_constructors() {
    let fixture = r#"(module
      (import "env" "memory" (memory i64 2 2 shared))
      (import "env" "wasm_input_publish_js" (func $publish (param i64)))
      (func (export "__wasm_call_ctors") i64.const 1024 call $publish)
      (func (export "_emscripten_stack_alloc") (param i64) (result i64) i64.const 8192)
      (func (export "__main_argc_argv") (param i32 i64) (result i32)
        i64.const 1464 i32.load))"#;
    for (mode, expected) in [
        (StartMode::Menu, 0),
        (StartMode::Story, 1),
        (StartMode::Sandbox, 2),
        (StartMode::SandboxEnvTest, 3),
    ] {
        let mut host = engine::load(&wat::parse_str(fixture).unwrap(), 2, 131072).unwrap();
        let mut launch = Launch::new(1280, 720);
        launch.start_mode = mode;
        assert_eq!(host.start_with_launch(&launch).unwrap(), expected);
    }
}

#[test]
fn a_concurrent_choice_is_not_lost_during_publication() {
    let mut host = engine::load(&wat::parse_str(FIXTURE).unwrap(), 2, 131072).unwrap();
    let producer = host.host.clone();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let ready = barrier.clone();
    let update = std::thread::spawn(move || {
        ready.wait();
        producer.set_start_mode(StartMode::Story).unwrap();
    });
    let publish = host
        .instance
        .get_typed_func::<i64, ()>(&mut host.store, "publish")
        .unwrap();
    barrier.wait();
    publish.call(&mut host.store, 1024).unwrap();
    update.join().unwrap();
    assert_eq!(
        InputBlock::new(host.memory.clone(), 1024)
            .unwrap()
            .start_mode()
            .unwrap(),
        1
    );
}

#[test]
fn rejects_invalid_blocks_before_announcing_publication() {
    let bytes = wat::parse_str(FIXTURE).unwrap();
    for address in [0, 3, 131072 - 440, 0x1_0000_0400, u64::MAX - 3] {
        let mut host = engine::load(&bytes, 2, 131072).unwrap();
        let publish = host
            .instance
            .get_typed_func::<i64, ()>(&mut host.store, "publish")
            .unwrap();
        assert!(
            publish.call(&mut host.store, address as i64).is_err(),
            "ponteiro inválido aceito: {address:#x}"
        );
        assert_eq!(host.host.input_block.load(Ordering::Acquire), 0);
    }
}
