use crate::{
    audio::{self, Capture, Speaker},
    network::{self, Interface},
    protocol::{Codec, Message, NodeId, FRAME_BYTES, PORT},
    session::{Activity, ReplayWindow, CHIME_MS, PEER_TIMEOUT},
};
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const ROOM_KEY: &[u8; 32] = include_bytes!(concat!(env!("OUT_DIR"), "/room.key"));

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Offline,
    Ready,
    Preparing,
    Talking,
    Listening,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub phase: Phase,
    pub peer: Option<String>,
    pub error: Option<String>,
    pub network: NetworkView,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkView {
    pub interfaces: Vec<Interface>,
    pub manual_peer: Option<Ipv4Addr>,
    pub sent: u64,
    pub received: u64,
    pub authenticated: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Begin,
    End,
    SetPeer(Option<Ipv4Addr>),
    Shutdown,
}

pub struct Handle {
    pub commands: mpsc::Sender<Command>,
    pub view: Arc<Mutex<View>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Handle {
    pub fn start() -> Self {
        let (commands, receive) = mpsc::channel();
        let view = Arc::new(Mutex::new(View::default()));
        let state = view.clone();
        let worker = thread::spawn(move || {
            let result = (|| {
                let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, PORT))
                    .map_err(|e| format!("Не удалось открыть порт {PORT}: {e}"))?;
                socket.set_broadcast(true).map_err(|e| e.to_string())?;
                socket.set_ttl(1).map_err(|e| e.to_string())?;
                let driver = Driver::new(
                    socket,
                    Vec::new(),
                    ROOM_KEY,
                    NativeAudio::default(),
                    state.clone(),
                )?;
                driver.run(receive);
                Ok::<(), String>(())
            })();
            if let Err(error) = result {
                state.lock().unwrap().error = Some(error);
            }
        });
        Self {
            commands,
            view,
            worker: Some(worker),
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let deadline = Instant::now() + Duration::from_millis(800);
            while !worker.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                crate::logging::write("audio/network worker did not stop within 800 ms; process exit releases devices");
            }
        }
    }
}

trait AudioBackend {
    fn prepare_mic(&mut self) -> Result<(), String>;
    fn start_mic(&mut self) -> Result<(), String>;
    fn capture(&mut self) -> Result<Option<Vec<u8>>, String>;
    fn stop_mic(&mut self);
    fn play(&mut self, pcm: Vec<u8>) -> Result<(), String>;
    fn output_idle(&mut self) -> bool;
    fn collect(&mut self);
    fn reset(&mut self);
}

#[derive(Default)]
struct NativeAudio {
    capture: Option<Capture>,
    speaker: Option<Speaker>,
}

impl AudioBackend for NativeAudio {
    fn prepare_mic(&mut self) -> Result<(), String> {
        self.capture = Some(Capture::open()?);
        Ok(())
    }
    fn start_mic(&mut self) -> Result<(), String> {
        self.capture.as_mut().ok_or("Микрофон не открыт")?.start()
    }
    fn capture(&mut self) -> Result<Option<Vec<u8>>, String> {
        match self.capture.as_mut() {
            Some(capture) => capture.poll(),
            None => Ok(None),
        }
    }
    fn stop_mic(&mut self) {
        self.capture = None;
    }
    fn play(&mut self, pcm: Vec<u8>) -> Result<(), String> {
        if self.speaker.is_none() {
            self.speaker = Some(Speaker::open()?);
        }
        self.speaker.as_mut().unwrap().play(pcm)
    }
    fn collect(&mut self) {
        if let Some(speaker) = &mut self.speaker {
            speaker.collect();
        }
    }
    fn output_idle(&mut self) -> bool {
        self.speaker.as_mut().is_none_or(Speaker::idle)
    }
    fn reset(&mut self) {
        if let Some(speaker) = &mut self.speaker {
            speaker.reset();
        }
    }
}

struct Peer {
    node: NodeId,
    address: SocketAddr,
    seen: Instant,
    replay: ReplayWindow,
}
struct Transmit {
    talk: u64,
    chime: bool,
    began: Instant,
    local_ready: Instant,
    acknowledged: bool,
    recording: bool,
    retry: Instant,
    sequence: u32,
}
struct Receive {
    talk: u64,
    chime: bool,
    ready_at: Instant,
    acknowledged: bool,
    seen: Instant,
    jitter: JitterBuffer,
    finish_at: Option<Instant>,
}

/// Sixty milliseconds absorbs normal Wi-Fi reordering. Gaps become silence.
#[derive(Default)]
struct JitterBuffer {
    frames: BTreeMap<u32, Vec<u8>>,
    next: u32,
    play_at: Option<Instant>,
}

impl JitterBuffer {
    fn push(&mut self, sequence: u32, pcm: Vec<u8>, now: Instant) {
        if sequence < self.next || sequence > self.next.saturating_add(100) {
            return;
        }
        if self.play_at.is_none() {
            self.play_at = Some(now + Duration::from_millis(60));
        }
        self.frames.entry(sequence).or_insert(pcm);
        // A stalled machine resumes near live audio instead of replaying seconds.
        if self.frames.len() > 12 {
            if let Some((&latest, _)) = self.frames.last_key_value() {
                self.next = latest.saturating_sub(2);
                self.frames.retain(|sequence, _| *sequence >= self.next);
                self.play_at = Some(now);
            }
        }
    }

    fn pop(&mut self, now: Instant) -> Option<Vec<u8>> {
        let play_at = self.play_at?;
        if now < play_at {
            return None;
        }
        let pcm = self
            .frames
            .remove(&self.next)
            .unwrap_or_else(|| vec![0; FRAME_BYTES]);
        self.next = self.next.saturating_add(1);
        self.play_at = Some(if now.duration_since(play_at) > Duration::from_millis(80) {
            now + Duration::from_millis(20)
        } else {
            play_at + Duration::from_millis(20)
        });
        Some(pcm)
    }
}

struct Driver<A> {
    socket: UdpSocket,
    destinations: Vec<SocketAddr>,
    codec: Codec,
    audio: A,
    view: Arc<Mutex<View>>,
    peer: Option<Peer>,
    tx: Option<Transmit>,
    rx: Option<Receive>,
    retired_talk: Option<u64>,
    activity: Activity,
    next_talk: u64,
    hello_at: Instant,
    refresh_at: Instant,
    fixed_destinations: bool,
    network: NetworkView,
    probes: Vec<SocketAddr>,
    probe_index: usize,
    probe_at: Instant,
    outgoing_interface: Option<u32>,
}

impl<A: AudioBackend> Driver<A> {
    fn new(
        socket: UdpSocket,
        destinations: Vec<SocketAddr>,
        key: &[u8; 32],
        audio: A,
        view: Arc<Mutex<View>>,
    ) -> Result<Self, String> {
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        let now = Instant::now();
        let fixed_destinations = !destinations.is_empty();
        Ok(Self {
            socket,
            fixed_destinations,
            destinations,
            codec: Codec::new(key)?,
            audio,
            view,
            peer: None,
            tx: None,
            rx: None,
            retired_talk: None,
            activity: Activity::default(),
            next_talk: 1,
            hello_at: now,
            refresh_at: now,
            network: NetworkView {
                manual_peer: if fixed_destinations {
                    None
                } else {
                    network::load_peer()
                },
                ..NetworkView::default()
            },
            probes: Vec::new(),
            probe_index: 0,
            probe_at: now,
            outgoing_interface: None,
        })
    }

    fn send_packet(&mut self, packet: &[u8], destination: SocketAddr) {
        let Some(ip) = network::destination_ip(destination) else {
            return;
        };
        let index = network::route(&self.network.interfaces, ip);
        if !self.fixed_destinations && index == 0 && !ip.is_loopback() {
            self.network.last_error =
                Some(format!("Адрес {ip} вне локальной сети Wi-Fi/Ethernet."));
            return;
        }
        let result = (|| {
            if self.outgoing_interface != Some(index) {
                network::set_interface(&self.socket, index)?;
                self.outgoing_interface = Some(index);
            }
            self.socket.send_to(packet, destination)
        })();
        match result {
            Ok(_) => {
                self.network.sent += 1;
            }
            Err(error) => {
                self.network.last_error = Some(format!("Отправка на {ip}: {error}"));
            }
        }
    }

    fn send(&mut self, message: Message) {
        if let Some(address) = self.peer.as_ref().map(|peer| peer.address) {
            let packet = self.codec.encode(&message);
            self.send_packet(&packet, address);
        }
    }

    fn end_transmit(&mut self) {
        self.audio.stop_mic();
        if let Some(tx) = self.tx.take() {
            for _ in 0..3 {
                self.send(Message::End { talk: tx.talk });
            }
        }
    }

    fn fail(&mut self, error: String) {
        self.end_transmit();
        self.rx = None;
        self.audio.reset();
        self.view.lock().unwrap().error = Some(error);
    }

    fn begin(&mut self, now: Instant) -> Result<(), String> {
        if self.peer.is_none() || self.tx.is_some() || self.rx.is_some() {
            return Ok(());
        }
        self.view.lock().unwrap().error = None;
        self.audio.prepare_mic()?;
        let chime = self.activity.needs_chime(now);
        if chime {
            self.audio.play(audio::chime())?;
        }
        let talk = self.next_talk;
        self.next_talk += 1;
        self.tx = Some(Transmit {
            talk,
            chime,
            began: now,
            local_ready: now + Duration::from_millis(if chime { CHIME_MS } else { 0 }),
            acknowledged: false,
            recording: false,
            retry: now,
            sequence: 0,
        });
        Ok(())
    }

    fn process(&mut self, message: Message, now: Instant) -> Result<(), String> {
        match message {
            Message::Hello => {}
            Message::Bye => {
                self.disconnect();
            }
            Message::Start { talk, chime } => {
                if self.retired_talk.is_some_and(|last| talk <= last) {
                    return Ok(());
                }
                if let Some(tx) = &self.tx {
                    // If both press together, the lexicographically smaller node wins.
                    if self.codec.node < self.peer.as_ref().unwrap().node {
                        return Ok(());
                    }
                    let _ = tx;
                    self.end_transmit();
                    self.audio.reset();
                }
                if let Some(rx) = &mut self.rx {
                    if rx.talk == talk {
                        rx.seen = now;
                        if rx.acknowledged {
                            let chime = rx.chime;
                            self.send(Message::Ready { talk, chime });
                        }
                        return Ok(());
                    }
                    // Ignore delayed starts for older utterances.
                    if talk < rx.talk {
                        return Ok(());
                    }
                }
                let chime = chime || self.activity.needs_chime(now);
                if chime {
                    self.audio.play(audio::chime())?;
                }
                // Ensure a broken/missing speaker is reported before allowing speech.
                else {
                    self.audio.play(vec![0; FRAME_BYTES])?;
                }
                self.rx = Some(Receive {
                    talk,
                    chime,
                    ready_at: now + Duration::from_millis(if chime { CHIME_MS } else { 0 }),
                    acknowledged: false,
                    seen: now,
                    jitter: JitterBuffer::default(),
                    finish_at: None,
                });
                self.view.lock().unwrap().error = None;
            }
            Message::Ready { talk, chime } => {
                if let Some(tx) = &mut self.tx {
                    if tx.talk == talk {
                        // A newly restarted receiver may need a warning even
                        // when the sender still considers the conversation warm.
                        if chime && !tx.chime {
                            self.audio.play(audio::chime())?;
                            tx.chime = true;
                            tx.local_ready = now + Duration::from_millis(CHIME_MS);
                        }
                        tx.acknowledged = true;
                    }
                }
            }
            Message::Audio {
                talk,
                sequence,
                pcm,
            } => {
                if let Some(rx) = &mut self.rx {
                    if rx.talk == talk && rx.acknowledged {
                        rx.seen = now;
                        rx.jitter.push(sequence, pcm, now);
                        self.activity.voice(now);
                    }
                }
            }
            Message::End { talk } => {
                if let Some(rx) = &mut self.rx {
                    if rx.talk == talk && rx.finish_at.is_none() {
                        rx.finish_at = Some(now + Duration::from_millis(140));
                    }
                } else {
                    self.retired_talk = Some(self.retired_talk.unwrap_or(0).max(talk));
                }
            }
        }
        Ok(())
    }

    fn disconnect(&mut self) {
        self.end_transmit();
        self.peer = None;
        self.rx = None;
        self.retired_talk = None;
        self.audio.reset();
    }

    fn publish(&mut self) {
        let mut view = self.view.lock().unwrap();
        view.phase = if self.tx.as_ref().is_some_and(|tx| tx.recording) {
            Phase::Talking
        } else if self.tx.is_some() {
            Phase::Preparing
        } else if self.rx.is_some() {
            Phase::Listening
        } else if self.peer.is_some() {
            Phase::Ready
        } else {
            Phase::Offline
        };
        view.peer = self.peer.as_ref().map(|peer| peer.address.ip().to_string());
        view.network = self.network.clone();
    }

    fn tick(&mut self, now: Instant) -> Result<(), String> {
        if now >= self.refresh_at && !self.fixed_destinations {
            match network::interfaces() {
                Ok(interfaces) => {
                    if interfaces != self.network.interfaces {
                        self.outgoing_interface = None;
                        self.probes = network::targets(&interfaces);
                        self.probe_index = 0;
                    }
                    self.destinations = interfaces
                        .iter()
                        .map(|interface| SocketAddr::from((interface.broadcast(), PORT)))
                        .collect();
                    self.network.interfaces = interfaces;
                }
                Err(error) => self.network.last_error = Some(format!("Сетевые адаптеры: {error}")),
            }
            self.refresh_at = now + Duration::from_secs(10);
        }
        if now >= self.hello_at {
            let packet = self.codec.encode(&Message::Hello);
            let destinations = match self.network.manual_peer {
                Some(ip) => vec![SocketAddr::from((ip, PORT))],
                None => self.destinations.clone(),
            };
            for destination in destinations {
                self.send_packet(&packet, destination);
            }
            self.send(Message::Hello);
            self.hello_at = now + Duration::from_secs(1);
        }
        // Some VPNs and routers drop broadcasts. A bounded, slow unicast Hello
        // pass discovers this radio on the local /24 without the VPN default route.
        if self.peer.is_none()
            && self.network.manual_peer.is_none()
            && now >= self.probe_at
            && !self.probes.is_empty()
        {
            let target = self.probes[self.probe_index];
            self.probe_index = (self.probe_index + 1) % self.probes.len();
            let packet = self.codec.encode(&Message::Hello);
            self.send_packet(&packet, target);
            self.probe_at = now + Duration::from_millis(25);
        }
        if self
            .peer
            .as_ref()
            .is_some_and(|peer| now.duration_since(peer.seen) >= PEER_TIMEOUT)
        {
            self.disconnect();
        }
        let mut bytes = [0_u8; 1500];
        for _ in 0..64 {
            let (length, source) = match self.socket.recv_from(&mut bytes) {
                Ok(packet) => packet,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
                Err(e) => return Err(format!("Сеть: {e}")),
            };
            self.network.received += 1;
            let Some((node, counter, message)) = self.codec.decode(&bytes[..length]) else {
                continue;
            };
            if node == self.codec.node {
                continue;
            }
            let Some(ip) = network::destination_ip(source) else {
                continue;
            };
            if !self.fixed_destinations
                && !ip.is_loopback()
                && network::route(&self.network.interfaces, ip) == 0
            {
                continue;
            }
            if self.network.manual_peer.is_some_and(|manual| manual != ip) {
                continue;
            }
            self.network.authenticated += 1;
            if self.peer.is_none() && message == Message::Hello {
                self.peer = Some(Peer {
                    node,
                    address: source,
                    seen: now,
                    replay: ReplayWindow::default(),
                });
                self.send(Message::Hello);
            }
            let Some(peer) = &mut self.peer else {
                continue;
            };
            if peer.node != node || peer.address != source || !peer.replay.accept(counter) {
                continue;
            }
            peer.seen = now;
            self.process(message, now)?;
        }
        if self.tx.as_ref().is_some_and(|tx| {
            !tx.recording && now.duration_since(tx.began) > Duration::from_secs(4)
        }) {
            return Err("Второй компьютер не ответил. Проверьте звук и брандмауэр на нём.".into());
        }
        if let Some(tx) = &mut self.tx {
            if !tx.acknowledged && now >= tx.retry {
                let message = Message::Start {
                    talk: tx.talk,
                    chime: tx.chime,
                };
                tx.retry = now + Duration::from_millis(100);
                self.send(message);
            }
        }
        if let Some(tx) = &mut self.tx {
            if tx.acknowledged && !tx.recording && now >= tx.local_ready && self.audio.output_idle()
            {
                self.audio.start_mic()?;
                tx.recording = true;
            }
        }
        for _ in 0..8 {
            if !self.tx.as_ref().is_some_and(|tx| tx.recording) {
                break;
            }
            let Some(pcm) = self.audio.capture()? else {
                break;
            };
            let tx = self.tx.as_mut().unwrap();
            let message = Message::Audio {
                talk: tx.talk,
                sequence: tx.sequence,
                pcm,
            };
            tx.sequence += 1;
            self.activity.voice(now);
            self.send(message);
        }
        if let Some(rx) = &mut self.rx {
            if !rx.acknowledged && now >= rx.ready_at && self.audio.output_idle() {
                rx.acknowledged = true;
                let talk = rx.talk;
                let chime = rx.chime;
                self.send(Message::Ready { talk, chime });
            }
        }
        if let Some(rx) = &mut self.rx {
            if let Some(pcm) = rx.jitter.pop(now) {
                self.audio.play(pcm)?;
            }
        }
        let finished = self.rx.as_ref().is_some_and(|rx| {
            rx.finish_at.is_some_and(|deadline| now >= deadline)
                || now.duration_since(rx.seen) >= Duration::from_millis(1200)
        });
        if finished {
            self.retired_talk = self.rx.take().map(|rx| rx.talk);
        }
        self.audio.collect();
        Ok(())
    }

    fn run(mut self, commands: mpsc::Receiver<Command>) {
        loop {
            let now = Instant::now();
            let mut shutdown = false;
            while let Ok(command) = commands.try_recv() {
                match command {
                    Command::Begin => {
                        if let Err(error) = self.begin(now) {
                            self.fail(error);
                        }
                    }
                    Command::End => self.end_transmit(),
                    Command::SetPeer(peer) => {
                        self.send(Message::Bye);
                        self.disconnect();
                        self.network.manual_peer = peer;
                        self.network.last_error = None;
                        self.refresh_at = now;
                        self.hello_at = now;
                        self.probe_at = now;
                    }
                    Command::Shutdown => {
                        shutdown = true;
                        break;
                    }
                }
            }
            if shutdown {
                break;
            }
            if let Err(error) = self.tick(now) {
                self.fail(error);
            }
            self.publish();
            thread::sleep(Duration::from_millis(3));
        }
        self.end_transmit();
        self.send(Message::Bye);
    }
}

// Exercises the real UDP transport and state machine without touching audio,
// startup settings, the tray, or any files other than the requested report.
pub fn self_test() -> String {
    match loopback_test() {
        Ok(()) => "PASS: encrypted unicast discovery without broadcasts, three-tone handshake, bidirectional PCM, release, and disconnect. No microphone or registry access.\r\n".into(),
        Err(error) => format!("FAIL: {error}\r\n"),
    }
}

#[derive(Default)]
struct Probe {
    tones: usize,
    frames: usize,
    recording: bool,
    first_capture: Option<Instant>,
    output_busy: bool,
}
struct TestAudio {
    probe: Arc<Mutex<Probe>>,
    next: Instant,
}
impl AudioBackend for TestAudio {
    fn prepare_mic(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn start_mic(&mut self) -> Result<(), String> {
        let mut probe = self.probe.lock().unwrap();
        probe.recording = true;
        probe.first_capture = Some(Instant::now());
        Ok(())
    }
    fn capture(&mut self) -> Result<Option<Vec<u8>>, String> {
        let now = Instant::now();
        if !self.probe.lock().unwrap().recording || now < self.next {
            return Ok(None);
        }
        self.next = now + Duration::from_millis(20);
        Ok(Some(vec![42; FRAME_BYTES]))
    }
    fn stop_mic(&mut self) {
        self.probe.lock().unwrap().recording = false;
    }
    fn play(&mut self, pcm: Vec<u8>) -> Result<(), String> {
        let mut probe = self.probe.lock().unwrap();
        if pcm.len() > FRAME_BYTES {
            probe.tones += 1;
        } else if pcm == vec![42; FRAME_BYTES] {
            probe.frames += 1;
        }
        Ok(())
    }
    fn collect(&mut self) {}
    fn output_idle(&mut self) -> bool {
        !self.probe.lock().unwrap().output_busy
    }
    fn reset(&mut self) {}
}

fn wait_until(condition: impl Fn() -> bool, timeout: Duration, label: &str) -> Result<(), String> {
    let until = Instant::now() + timeout;
    while Instant::now() < until {
        if condition() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(format!("Timed out: {label}"))
}

fn loopback_test() -> Result<(), String> {
    let a = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    let b = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    let address_a = a.local_addr().unwrap();
    let address_b = b.local_addr().unwrap();
    let views = [
        Arc::new(Mutex::new(View::default())),
        Arc::new(Mutex::new(View::default())),
    ];
    let probes = [
        Arc::new(Mutex::new(Probe::default())),
        Arc::new(Mutex::new(Probe::default())),
    ];
    let mut workers = Vec::new();
    let mut controls = Vec::new();
    for (index, (socket, target)) in [(a, address_b), (b, address_a)].into_iter().enumerate() {
        let audio = TestAudio {
            probe: probes[index].clone(),
            next: Instant::now(),
        };
        let mut driver = Driver::new(socket, vec![target], &[91; 32], audio, views[index].clone())?;
        // No broadcast or ordinary Hello destination is available in this scenario.
        // Discovery must use the same fallback probe path as a VPN-filtered LAN.
        driver.destinations.clear();
        driver.probes = vec![target];
        let (send, receive) = mpsc::channel();
        workers.push(thread::spawn(move || driver.run(receive)));
        controls.push(send);
    }
    let timeout = Duration::from_secs(4);
    let result = (|| {
        wait_until(
            || {
                views
                    .iter()
                    .all(|v| v.lock().unwrap().phase == Phase::Ready)
            },
            timeout,
            "discovery",
        )?;
        let began = Instant::now();
        controls[0].send(Command::Begin).unwrap();
        wait_until(
            || probes[1].lock().unwrap().frames >= 5,
            timeout,
            "A to B audio",
        )?;
        for probe in &probes {
            if probe.lock().unwrap().tones != 1 {
                return Err("First call must chime on both PCs".into());
            }
        }
        if probes[0]
            .lock()
            .unwrap()
            .first_capture
            .unwrap()
            .duration_since(began)
            < Duration::from_millis(CHIME_MS)
        {
            return Err("Microphone started before warning finished".into());
        }
        controls[0].send(Command::End).unwrap();
        wait_until(
            || {
                views
                    .iter()
                    .all(|v| v.lock().unwrap().phase == Phase::Ready)
            },
            timeout,
            "release",
        )?;
        if probes[0].lock().unwrap().recording {
            return Err("Microphone stayed open after release".into());
        }
        controls[1].send(Command::Begin).unwrap();
        wait_until(
            || probes[0].lock().unwrap().frames >= 5,
            timeout,
            "B to A audio",
        )?;
        if probes.iter().any(|p| p.lock().unwrap().tones != 1) {
            return Err("Warm conversation chimed again".into());
        }
        controls[1].send(Command::End).unwrap();
        wait_until(
            || {
                views
                    .iter()
                    .all(|v| v.lock().unwrap().phase == Phase::Ready)
            },
            timeout,
            "reverse release",
        )?;
        controls[1].send(Command::Shutdown).unwrap();
        wait_until(
            || views[0].lock().unwrap().phase == Phase::Offline,
            timeout,
            "disconnect",
        )?;
        Ok(())
    })();
    for control in controls {
        let _ = control.send(Command::Shutdown);
    }
    for worker in workers {
        let _ = worker.join();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(node: u8, other: u8) -> (Driver<TestAudio>, Arc<Mutex<Probe>>, UdpSocket) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let remote = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = remote.local_addr().unwrap();
        let probe = Arc::new(Mutex::new(Probe::default()));
        let audio = TestAudio {
            probe: probe.clone(),
            next: Instant::now(),
        };
        let mut driver = Driver::new(
            socket,
            vec![address],
            &[7; 32],
            audio,
            Arc::new(Mutex::new(View::default())),
        )
        .unwrap();
        driver.codec.node = [node; 16];
        driver.peer = Some(Peer {
            node: [other; 16],
            address,
            seen: Instant::now(),
            replay: ReplayWindow::default(),
        });
        (driver, probe, remote)
    }

    #[test]
    fn two_radios_discover_warn_and_exchange_audio_in_both_directions() {
        loopback_test().unwrap();
    }

    #[test]
    fn direct_connection_ignores_other_authenticated_radios() {
        let (mut driver, _, remote) = fixture(1, 2);
        driver.peer = None;
        driver.network.manual_peer = Some(Ipv4Addr::new(192, 168, 0, 25));
        let mut other = Codec::new(&[7; 32]).unwrap();
        remote
            .send_to(
                &other.encode(&Message::Hello),
                driver.socket.local_addr().unwrap(),
            )
            .unwrap();
        driver.tick(Instant::now()).unwrap();
        assert!(driver.peer.is_none());
        assert_eq!(driver.network.received, 1);
        assert_eq!(driver.network.authenticated, 0);
    }

    #[test]
    fn lost_ready_and_repeated_starts_do_not_repeat_the_warning() {
        let (mut driver, probe, _remote) = fixture(2, 1);
        let now = Instant::now();
        driver
            .process(
                Message::Start {
                    talk: 1,
                    chime: true,
                },
                now,
            )
            .unwrap();
        driver
            .process(
                Message::Start {
                    talk: 1,
                    chime: true,
                },
                now + Duration::from_millis(100),
            )
            .unwrap();
        assert!(!driver.rx.as_ref().unwrap().acknowledged);
        driver.tick(now + Duration::from_millis(480)).unwrap();
        assert!(driver.rx.as_ref().unwrap().acknowledged);
        driver
            .process(
                Message::Start {
                    talk: 1,
                    chime: true,
                },
                now + Duration::from_millis(500),
            )
            .unwrap();
        assert_eq!(probe.lock().unwrap().tones, 1);
        assert!(!probe.lock().unwrap().recording);
    }

    #[test]
    fn a_restarted_receiver_can_request_a_warning_on_both_sides() {
        let (mut driver, probe, _remote) = fixture(1, 2);
        let now = Instant::now();
        driver.activity.voice(now);
        driver.begin(now).unwrap();
        assert_eq!(probe.lock().unwrap().tones, 0);
        driver
            .process(
                Message::Ready {
                    talk: 1,
                    chime: true,
                },
                now + Duration::from_millis(480),
            )
            .unwrap();
        driver.tick(now + Duration::from_millis(959)).unwrap();
        assert!(!probe.lock().unwrap().recording);
        driver.tick(now + Duration::from_millis(960)).unwrap();
        assert!(probe.lock().unwrap().recording);
        assert_eq!(probe.lock().unwrap().tones, 1);
        driver.end_transmit();
        assert!(!probe.lock().unwrap().recording);
    }

    #[test]
    fn slow_speaker_must_finish_the_warning_before_acknowledgement_or_capture() {
        let (mut receiver, receiver_probe, _remote_a) = fixture(2, 1);
        let (mut sender, sender_probe, _remote_b) = fixture(1, 2);
        let now = Instant::now();
        receiver_probe.lock().unwrap().output_busy = true;
        receiver
            .process(
                Message::Start {
                    talk: 1,
                    chime: true,
                },
                now,
            )
            .unwrap();
        receiver.tick(now + Duration::from_millis(600)).unwrap();
        assert!(!receiver.rx.as_ref().unwrap().acknowledged);
        receiver_probe.lock().unwrap().output_busy = false;
        receiver.tick(now + Duration::from_millis(650)).unwrap();
        assert!(receiver.rx.as_ref().unwrap().acknowledged);

        sender.begin(now).unwrap();
        sender_probe.lock().unwrap().output_busy = true;
        sender
            .process(
                Message::Ready {
                    talk: 1,
                    chime: true,
                },
                now + Duration::from_millis(650),
            )
            .unwrap();
        sender.tick(now + Duration::from_millis(650)).unwrap();
        assert!(!sender_probe.lock().unwrap().recording);
        sender_probe.lock().unwrap().output_busy = false;
        sender.tick(now + Duration::from_millis(700)).unwrap();
        assert!(sender_probe.lock().unwrap().recording);
    }

    #[test]
    fn simultaneous_presses_choose_exactly_one_sender() {
        let (mut winner, _, _remote_a) = fixture(1, 2);
        let (mut loser, probe, _remote_b) = fixture(2, 1);
        let now = Instant::now();
        winner.begin(now).unwrap();
        loser.begin(now).unwrap();
        winner
            .process(
                Message::Start {
                    talk: 1,
                    chime: true,
                },
                now,
            )
            .unwrap();
        loser
            .process(
                Message::Start {
                    talk: 1,
                    chime: true,
                },
                now,
            )
            .unwrap();
        assert!(winner.tx.is_some() && winner.rx.is_none());
        assert!(loser.tx.is_none() && loser.rx.is_some());
        assert!(!probe.lock().unwrap().recording);
    }

    #[test]
    fn peer_timeout_closes_capture_and_removes_presence() {
        let (mut driver, probe, _remote) = fixture(1, 2);
        let now = Instant::now();
        driver.activity.voice(now);
        driver.begin(now).unwrap();
        driver
            .process(
                Message::Ready {
                    talk: 1,
                    chime: false,
                },
                now,
            )
            .unwrap();
        driver.tick(now).unwrap();
        assert!(probe.lock().unwrap().recording);
        driver.tick(now + PEER_TIMEOUT).unwrap();
        assert!(!probe.lock().unwrap().recording);
        assert!(driver.peer.is_none());
    }

    #[test]
    fn canceled_utterance_cannot_be_reopened_by_a_late_start() {
        let (mut driver, probe, _remote) = fixture(2, 1);
        let now = Instant::now();
        driver.process(Message::End { talk: 1 }, now).unwrap();
        driver
            .process(
                Message::Start {
                    talk: 1,
                    chime: true,
                },
                now,
            )
            .unwrap();
        assert!(driver.rx.is_none());
        assert_eq!(probe.lock().unwrap().tones, 0);
    }

    #[test]
    #[ignore = "requires the release GUI running with --no-autostart and no second peer"]
    fn native_window_reopens_after_explicit_tray_hide_on_a_second_launch() {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            FindWindowW, IsWindowVisible, PostMessageW, WM_COMMAND,
        };
        let class: Vec<u16> = "WifiRadioWindow".encode_utf16().chain(Some(0)).collect();
        let window = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        assert!(
            !window.is_null(),
            "Start dist/WifiRadio.exe --no-autostart first"
        );
        for _ in 0..3 {
            unsafe {
                PostMessageW(window, WM_COMMAND, 205, 0);
            }
            wait_until(
                || unsafe { IsWindowVisible(window) } == 0,
                Duration::from_secs(2),
                "explicit tray action hides the radio",
            )
            .unwrap();
            let mut duplicate = std::process::Command::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/dist/WifiRadio.exe"
            ))
            .args(["--no-autostart", "--background"])
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
            wait_until(
                || unsafe { IsWindowVisible(window) } != 0,
                Duration::from_secs(3),
                "a second launch restores the hidden window",
            )
            .unwrap();
            assert!(duplicate.wait().unwrap().success());
            assert_eq!(
                unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) },
                window
            );
        }
    }

    #[test]
    #[ignore = "requires the release GUI running with --no-autostart and no second peer"]
    fn native_window_follows_real_peer_discovery_and_disconnect() {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            FindWindowW, GetDlgItem, GetWindowTextW,
        };
        let class: Vec<u16> = "WifiRadioWindow".encode_utf16().chain(Some(0)).collect();
        let window = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        assert!(
            !window.is_null(),
            "Start dist/WifiRadio.exe --no-autostart first"
        );
        let button = unsafe { GetDlgItem(window, 100) };
        assert!(!button.is_null());
        assert_eq!(
            unsafe { IsWindowEnabled(button) },
            0,
            "Another real peer is already connected"
        );
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let probe = Arc::new(Mutex::new(Probe::default()));
        let audio = TestAudio {
            probe: probe.clone(),
            next: Instant::now(),
        };
        let driver = Driver::new(
            socket,
            vec![SocketAddr::from((Ipv4Addr::LOCALHOST, PORT))],
            ROOM_KEY,
            audio,
            Arc::new(Mutex::new(View::default())),
        )
        .unwrap();
        let (commands, receive) = mpsc::channel();
        let worker = thread::spawn(move || driver.run(receive));
        let discovered = wait_until(
            || unsafe { IsWindowEnabled(button) } != 0,
            Duration::from_secs(4),
            "native push-to-talk enabled by authenticated discovery",
        );
        let mut label = [0_u16; 100];
        let length = unsafe { GetWindowTextW(button, label.as_mut_ptr(), label.len() as i32) };
        let _ = commands.send(Command::Shutdown);
        worker.join().unwrap();
        discovered.unwrap();
        assert_eq!(
            String::from_utf16_lossy(&label[..length as usize]),
            "Держать и говорить"
        );
        wait_until(
            || unsafe { IsWindowEnabled(button) } == 0,
            Duration::from_secs(4),
            "native push-to-talk disabled after Bye",
        )
        .unwrap();
        assert_eq!(
            probe.lock().unwrap().tones,
            0,
            "Presence alone must not make sound"
        );
    }

    #[test]
    fn jitter_buffer_reorders_packets_and_conceals_loss() {
        let now = Instant::now();
        let mut jitter = JitterBuffer::default();
        jitter.push(1, vec![2; FRAME_BYTES], now);
        jitter.push(0, vec![1; FRAME_BYTES], now);
        assert!(jitter.pop(now).is_none());
        assert_eq!(
            jitter.pop(now + Duration::from_millis(60)).unwrap(),
            vec![1; FRAME_BYTES]
        );
        assert_eq!(
            jitter.pop(now + Duration::from_millis(80)).unwrap(),
            vec![2; FRAME_BYTES]
        );
        assert_eq!(
            jitter.pop(now + Duration::from_millis(100)).unwrap(),
            vec![0; FRAME_BYTES]
        );
        jitter.push(0, vec![9; FRAME_BYTES], now);
        assert!(jitter.frames.is_empty());
    }
}
