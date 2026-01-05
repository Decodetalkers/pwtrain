use std::{cell::RefCell, rc::Rc, sync::Mutex};

use pipewire::{
    self as pw,
    node::Node,
    spa::{
        pod::deserialize::PodDeserializer,
        utils::{dict::DictRef, result::AsyncSeq},
    },
};

#[derive(Clone, Debug)]
enum Direction {
    Input,
    Output,
}

#[derive(Clone, Debug)]
pub struct Device {
    id: u32,
    node_name: String,
    nick_name: String,
    description: String,
    direction: Direction,
    channels: usize,
    buffer_limit: u32,
}

impl Device {}

fn roundtrip() -> Option<Vec<Device>> {
    let mainloop = pw::main_loop::MainLoopRc::new(None).ok()?;
    let context = pw::context::ContextRc::new(&mainloop, None).ok()?;
    let core = context.connect_rc(None).ok()?;
    let registry = core.get_registry_rc().ok()?;

    // To comply with Rust's safety rules, we wrap this variable in an `Rc` and  a `Cell`.
    let devices: Rc<RefCell<Vec<Device>>> = Rc::new(RefCell::new(vec![]));
    let requests = Rc::new(Mutex::new(vec![]));
    let loop_clone = mainloop.clone();

    // Trigger the sync event. The server's answer won't be processed until we start the main loop,
    // so we can safely do this before setting up a callback. This lets us avoid using a Cell.
    let peddings: Rc<RefCell<Vec<AsyncSeq>>> = Rc::new(RefCell::new(vec![]));
    let pending = core.sync(0).expect("sync failed");

    peddings.borrow_mut().push(pending);

    let _listener_core = core
        .add_listener_local()
        .done({
            let peddings = peddings.clone();
            move |id, seq| {
                if id != pw::core::PW_ID_CORE {
                    return;
                }
                let mut peddinglist = peddings.borrow_mut();
                let Some(index) = peddinglist.iter().position(|o_seq| *o_seq == seq) else {
                    return;
                };
                peddinglist.remove(index);
                if !peddinglist.is_empty() {
                    return;
                }
                loop_clone.quit();
            }
        })
        .register();
    let _listener_reg = registry
        .add_listener_local()
        .global({
            let devices = devices.clone();
            let registry = registry.clone();
            let settings = requests.clone();
            move |global| match global.type_ {
                pipewire::types::ObjectType::Node => {
                    let Some(props) = global.props else {
                        return;
                    };
                    let Some(media_class) = props.get("media.class") else {
                        return;
                    };
                    match media_class {
                        "Audio/Sink" => Direction::Input,
                        "Audio/Source" => Direction::Output,
                        _ => {
                            return;
                        }
                    };
                    let node: Node = registry.bind(global).expect("should ok");

                    let devices = devices.clone();
                    let listener = node
                        .add_listener_local()
                        .info(move |info| {
                            let Some(props) = info.props() else {
                                return;
                            };
                            dbg!(props);
                            let Some(media_class) = props.get("media.class") else {
                                return;
                            };
                            let direction = match media_class {
                                "Audio/Sink" => Direction::Input,
                                "Audio/Source" => Direction::Output,
                                _ => {
                                    return;
                                }
                            };
                            let id = info.id();
                            let node_name = props.get("node.name").unwrap_or("unknown").to_owned();
                            let nick_name = props.get("node.nick").unwrap_or("unknown").to_owned();
                            let description = props
                                .get("node.description")
                                .unwrap_or("unknown")
                                .to_owned();
                            let channels: usize = props
                                .get("audio.channels")
                                .and_then(|channels| channels.parse().ok())
                                .unwrap_or(2);
                            let buffer_limit: u32 = props
                                .get("clock.quantum-limit")
                                .and_then(|channels| channels.parse().ok())
                                .unwrap_or(0);
                            let device = Device {
                                id,
                                node_name,
                                nick_name,
                                description,
                                direction,
                                channels,
                                buffer_limit,
                            };
                            devices.borrow_mut().push(device);
                        })
                        .register();
                    let pending = core.sync(0).expect("sync failed");
                    peddings.borrow_mut().push(pending);
                    settings.lock().unwrap().push((node, listener));
                }
                _ => {}
            }
        })
        .register();

    mainloop.run();

    let devices = devices.borrow().clone();
    Some(devices)
}

fn main() {
    pw::init();
    let devices = roundtrip().unwrap();
    println!("devices {devices:?}");
    unsafe {
        pw::deinit();
    }
}
