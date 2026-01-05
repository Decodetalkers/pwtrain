use std::{cell::RefCell, rc::Rc};

use pipewire::{
    self as pw,
    metadata::{Metadata, MetadataListener},
    node::{Node, NodeListener},
    proxy::ProxyT,
    spa::utils::result::AsyncSeq,
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

#[derive(Debug, Clone, Default)]
struct Settings {
    rate: u32,
    allow_rates: Vec<u32>,
    quantum: u32,
    min_quantum: u32,
    max_quantum: u32,
}

#[allow(dead_code)]
enum Request {
    Node(NodeListener),
    Meta(MetadataListener),
}

impl From<NodeListener> for Request {
    fn from(value: NodeListener) -> Self {
        Self::Node(value)
    }
}

impl From<MetadataListener> for Request {
    fn from(value: MetadataListener) -> Self {
        Self::Meta(value)
    }
}

#[derive(Debug, Clone, Default)]
struct InitResult {
    devices: Vec<Device>,
    settings: Settings,
}

fn init_roundtrip() -> Option<InitResult> {
    let mainloop = pw::main_loop::MainLoopRc::new(None).ok()?;
    let context = pw::context::ContextRc::new(&mainloop, None).ok()?;
    let core = context.connect_rc(None).ok()?;
    let registry = core.get_registry_rc().ok()?;

    // To comply with Rust's safety rules, we wrap this variable in an `Rc` and  a `Cell`.
    let devices: Rc<RefCell<Vec<Device>>> = Rc::new(RefCell::new(vec![]));
    let requests = Rc::new(RefCell::new(vec![]));
    let settings = Rc::new(RefCell::new(Settings::default()));
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
            let requests = requests.clone();
            let settings = settings.clone();
            move |global| match global.type_ {
                pipewire::types::ObjectType::Metadata => {
                    if !global.props.is_some_and(|props| {
                        props
                            .get("metadata.name")
                            .is_some_and(|name| name == "settings")
                    }) {
                        return;
                    }
                    let meta_settings: Metadata = registry.bind(global).unwrap();
                    let settings = settings.clone();
                    let listener = meta_settings
                        .add_listener_local()
                        .property(move |_, key, _, value| {
                            match (key, value) {
                                (Some("clock.rate"), Some(rate)) => {
                                    let Ok(rate) = rate.parse() else {
                                        return 0;
                                    };
                                    settings.borrow_mut().rate = rate;
                                }
                                (Some("clock.allowed-rates"), Some(list)) => {
                                    let Some(list) = list.strip_prefix("[") else {
                                        return 0;
                                    };
                                    let Some(list) = list.strip_suffix("]") else {
                                        return 0;
                                    };
                                    let list = list.trim();
                                    let list: Vec<&str> = list.split(' ').collect();
                                    let mut allow_rates = vec![];
                                    for rate in list {
                                        let Ok(rate) = rate.parse() else {
                                            return 0;
                                        };
                                        allow_rates.push(rate);
                                    }
                                    settings.borrow_mut().allow_rates = allow_rates;
                                }
                                (Some("clock.quantum"), Some(quantum)) => {
                                    let Ok(quantum) = quantum.parse() else {
                                        return 0;
                                    };
                                    settings.borrow_mut().quantum = quantum;
                                }
                                (Some("clock.min-quantum"), Some(min_quantum)) => {
                                    let Ok(min_quantum) = min_quantum.parse() else {
                                        return 0;
                                    };
                                    settings.borrow_mut().min_quantum = min_quantum;
                                }
                                (Some("clock.max-quantum"), Some(max_quantum)) => {
                                    let Ok(max_quantum) = max_quantum.parse() else {
                                        return 0;
                                    };
                                    settings.borrow_mut().max_quantum = max_quantum;
                                }
                                _ => {}
                            }
                            0
                        })
                        .register();
                    let pending = core.sync(0).expect("sync failed");
                    peddings.borrow_mut().push(pending);
                    requests
                        .borrow_mut()
                        .push((meta_settings.upcast(), Request::Meta(listener)));
                }
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
                    requests
                        .borrow_mut()
                        .push((node.upcast(), Request::Node(listener)));
                }
                _ => {}
            }
        })
        .register();

    mainloop.run();

    let devices = devices.take();
    let settings = settings.take();
    Some(InitResult { devices, settings })
}

fn main() {
    pw::init();
    let InitResult { devices, settings } = init_roundtrip().unwrap();
    println!("devices {devices:?}");
    println!("settings {settings:?}");
    unsafe {
        pw::deinit();
    }
}
