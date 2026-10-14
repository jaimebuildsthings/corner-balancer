//! The web UI on the host pad: its own Wi-Fi network, DHCP, a captive portal
//! (every DNS name leads here), and an HTTP server for `web/index.html` with a
//! WebSocket at `/ws`.
//!
//! The portal works like hotel Wi-Fi. Until someone taps Continue on the
//! welcome page, phones' connectivity checks get the welcome page, so the
//! phone pops it up. After that the checks get the answer each OS expects, so
//! the phone keeps the network and the full UI can be used in a real browser
//! at http://192.168.4.1/ (the pop-up sheet can't download CSV files).
//!
//! The WebSocket sends scales-protocol frames (as with `json on`) and accepts
//! console commands as text messages, so the page and the serial console share
//! one command language.

use core::cell::RefCell;
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_executor::Spawner;
use embassy_net::{Runner, Stack, StackResources, StaticConfigV4};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{Interface, WifiController};
use picoserve::futures::Either;
use picoserve::response::ws;
use picoserve::response::{File, NoContent, ResponseWriter, StatusCode};
use picoserve::routing::{PathRouterService, RequestHandlerService, get};
use picoserve::{AppBuilder, AppRouter, ResponseSent};
use static_cell::StaticCell;

use crate::out::say;

/// The host pad's address on its own network.
pub const ADDRESS: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);

/// The UI page, embedded at build time.
const PAGE: Html = Html(include_str!("../../../web/index.html"));

/// Shown by phones' captive-portal pop-up until someone continues.
const WELCOME: Html = Html(include_str!("../../../web/welcome.html"));

/// Apple's expected answer to its connectivity check.
const APPLE_SUCCESS: Html = Html("<HTML><HEAD><TITLE>Success</TITLE></HEAD><BODY>Success</BODY></HTML>");

/// An HTML body that handlers can pick at runtime.
#[derive(Clone, Copy)]
struct Html(&'static str);

impl picoserve::response::Content for Html {
    fn content_type(&self) -> &'static str {
        "text/html; charset=utf-8"
    }

    fn content_length(&self) -> usize {
        self.0.len()
    }

    async fn write_content<W: picoserve::io::Write>(self, writer: W) -> Result<(), W::Error> {
        self.0.write_content(writer).await
    }
}

/// Someone tapped Continue on the welcome page since the pad started.
static PORTAL_DONE: AtomicBool = AtomicBool::new(false);

fn portal_done() -> bool {
    PORTAL_DONE.load(Ordering::Relaxed)
}

/// Longest frame passed to WebSocket clients.
const FRAME_MAX: usize = 1024;

/// HTTP connections served at once (each is one task with its own buffers).
const WEB_TASKS: usize = 3;

/// Sockets: one per web task, plus DHCP and captive DNS. (embassy-net would add
/// one more for its own DNS client if its `dns` feature were on; it's kept off.)
const SOCKETS: usize = WEB_TASKS + 2;

struct Frame {
    buf: [u8; FRAME_MAX],
    len: usize,
    seq: u32,
}

static FRAME: Mutex<CriticalSectionRawMutex, RefCell<Frame>> =
    Mutex::new(RefCell::new(Frame { buf: [0; FRAME_MAX], len: 0, seq: 0 }));

/// Hands the latest scales-protocol frame to WebSocket clients.
pub fn publish(frame: &[u8]) {
    FRAME.lock(|f| {
        let mut f = f.borrow_mut();
        let n = frame.len().min(FRAME_MAX);
        f.buf[..n].copy_from_slice(&frame[..n]);
        f.len = n;
        f.seq = f.seq.wrapping_add(1);
    });
}

/// Copies the latest frame into `out`; returns its length and sequence number.
fn latest(out: &mut [u8; FRAME_MAX]) -> (usize, u32) {
    FRAME.lock(|f| {
        let f = f.borrow();
        out[..f.len].copy_from_slice(&f.buf[..f.len]);
        (f.len, f.seq)
    })
}

/// The network name, `CornerScales-` plus the pad's name, in a fixed buffer.
pub struct Ssid {
    buf: [u8; 32],
    len: usize,
}

impl Ssid {
    /// The text.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("CornerScales")
    }
}

impl core::fmt::Write for Ssid {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// The network name for the pad with this MAC address.
pub fn ssid(mac: &[u8; 6]) -> Ssid {
    let mut s = Ssid { buf: [0; 32], len: 0 };
    let _ = core::fmt::Write::write_fmt(&mut s, format_args!("CornerScales-{}", protocol::pad::PadName::from_mac(mac)));
    s
}

/// Starts the network stack and servers on the access-point interface.
pub fn start(spawner: Spawner, controller: WifiController<'static>, device: Interface, seed: u64) {
    let config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: embassy_net::Ipv4Cidr::new(ADDRESS, 24),
        gateway: Some(ADDRESS),
        dns_servers: Default::default(),
    });
    static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(device, config, RESOURCES.init(StackResources::new()), seed);

    static APP: StaticCell<AppRouter<WebApp>> = StaticCell::new();
    let app = APP.init(WebApp.build_app());

    spawner.spawn(wifi_task(controller).expect("wifi task"));
    spawner.spawn(net_task(runner).expect("net task"));
    spawner.spawn(dhcp_task(stack).expect("dhcp task"));
    spawner.spawn(dns_task(stack).expect("dns task"));
    for id in 0..WEB_TASKS {
        spawner.spawn(web_task(id, stack, app).expect("web task"));
    }
}

/// Keeps the controller (and so the access point) alive and reports phones joining.
#[embassy_executor::task]
async fn wifi_task(controller: WifiController<'static>) {
    use esp_radio::wifi::ap::EventInfo;
    loop {
        match controller.wait_for_access_point_connected_event_async().await {
            Ok(EventInfo::Connected(_)) => say!("web: a device joined the network"),
            Ok(EventInfo::Disconnected(_)) => say!("web: a device left the network"),
            _ => Timer::after_secs(1).await,
        }
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

/// Hands out addresses, naming this pad as router and DNS server, and points
/// phones at the page (RFC 8910 captive portal URL).
#[embassy_executor::task]
async fn dhcp_task(stack: Stack<'static>) {
    use edge_dhcp::io::{self, DEFAULT_SERVER_PORT};
    use edge_dhcp::server::{Server, ServerOptions};
    use edge_nal::UdpBind;
    use edge_nal_embassy::{Udp, UdpBuffers};

    let mut buf = [0u8; 1500];
    let mut gateway = [Ipv4Addr::UNSPECIFIED];
    let dns = [ADDRESS];
    let buffers = UdpBuffers::<1, 1024, 1024, 4>::new();
    let udp = Udp::new(stack, &buffers);
    let Ok(mut socket) = udp.bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_SERVER_PORT))).await
    else {
        say!("web: DHCP could not start");
        return;
    };
    loop {
        let mut options = ServerOptions::new(ADDRESS, Some(&mut gateway));
        options.dns = &dns;
        options.captive_url = Some("http://192.168.4.1/");
        let _ = io::server::run(&mut Server::<_, 8>::new_with_et(ADDRESS), &options, &mut socket, &mut buf).await;
        Timer::after_millis(500).await;
    }
}

/// Answers every DNS question with this pad's address.
#[embassy_executor::task]
async fn dns_task(stack: Stack<'static>) {
    use edge_nal_embassy::{Udp, UdpBuffers};

    let mut tx = [0u8; 512];
    let mut rx = [0u8; 512];
    let buffers = UdpBuffers::<1, 512, 512, 4>::new();
    let udp = Udp::new(stack, &buffers);
    loop {
        let _ = edge_captive::io::run(
            &udp,
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 53)),
            &mut tx,
            &mut rx,
            ADDRESS,
            core::time::Duration::from_secs(60),
        )
        .await;
        Timer::after_millis(500).await;
    }
}

static CONFIG: picoserve::Config = picoserve::Config::const_default().keep_connection_alive();

#[embassy_executor::task(pool_size = WEB_TASKS)]
async fn web_task(id: usize, stack: Stack<'static>, app: &'static AppRouter<WebApp>) -> ! {
    let mut tcp_rx = [0; 1024];
    let mut tcp_tx = [0; 2048];
    let mut http = [0; 2048];
    picoserve::Server::new(app, &CONFIG, &mut http)
        .listen_and_serve(id, stack, 80, &mut tcp_rx, &mut tcp_tx)
        .await
        .into_never()
}

struct WebApp;

impl AppBuilder for WebApp {
    type PathRouter = impl picoserve::routing::PathRouter;

    fn build_app(self) -> picoserve::Router<Self::PathRouter> {
        // Connectivity checks: the welcome page until continued, then what each OS expects.
        let apple = || async { if portal_done() { Ok(APPLE_SUCCESS) } else { Err(WELCOME) } };
        let android = || async { if portal_done() { Ok((StatusCode::NO_CONTENT, NoContent)) } else { Err(WELCOME) } };
        picoserve::Router::from_service(AnyPath)
            .route("/", get(async || PAGE))
            .route("/ws", get(async |upgrade: ws::WebSocketUpgrade| upgrade.on_upgrade(Live)))
            .route(
                "/portal/continue",
                get(async || {
                    PORTAL_DONE.store(true, Ordering::Relaxed);
                    "ok"
                }),
            )
            .route("/hotspot-detect.html", get(apple))
            .route("/library/test/success.html", get(apple))
            .route("/generate_204", get(android))
            .route("/gen_204", get(android))
            .route(
                "/connecttest.txt",
                get(async || if portal_done() { Ok("Microsoft Connect Test") } else { Err(WELCOME) }),
            )
            .route("/ncsi.txt", get(async || if portal_done() { Ok("Microsoft NCSI") } else { Err(WELCOME) }))
    }
}

/// Every other path (other OS checks, stray requests): the welcome page until
/// continued, then the UI.
struct AnyPath;

impl PathRouterService for AnyPath {
    async fn call_path_router_service<R: picoserve::io::Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        state: &(),
        current_path_parameters: (),
        _path: picoserve::request::Path<'_>,
        request: picoserve::request::Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        let page = if portal_done() { PAGE } else { WELCOME };
        File::html(page.0).call_request_handler_service(state, current_path_parameters, request, response_writer).await
    }
}

/// One phone's live connection: frames out, commands in.
struct Live;

impl ws::WebSocketCallback for Live {
    async fn run<R: picoserve::io::Read, W: picoserve::io::Write<Error = R::Error>>(
        self,
        mut rx: ws::SocketRx<R>,
        mut tx: ws::SocketTx<W>,
    ) -> Result<(), W::Error> {
        let mut inbox = [0u8; 128];
        let mut frame = [0u8; FRAME_MAX];
        let mut sent_seq = 0;
        loop {
            match rx.next_message(&mut inbox, Timer::after(Duration::from_millis(100))).await? {
                Either::First(Ok(ws::Message::Text(command))) => crate::console::submit(command),
                Either::First(Ok(ws::Message::Ping(data))) => tx.send_pong(data).await?,
                Either::First(Ok(ws::Message::Close(_))) => break,
                Either::First(Ok(_)) => {}
                Either::First(Err(e)) => return tx.close(Some((e.code(), "error"))).await,
                Either::Second(()) => {
                    let (len, seq) = latest(&mut frame);
                    if seq != sent_seq && len > 0 {
                        sent_seq = seq;
                        tx.send_text(core::str::from_utf8(&frame[..len]).unwrap_or("{}")).await?;
                    }
                }
            }
        }
        tx.close(None).await
    }
}
