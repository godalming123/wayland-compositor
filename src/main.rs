static POSSIBLE_BACKENDS: &[&str] = &[
    #[cfg(feature = "winit")]
    "winit : Run anvil as a X11 or Wayland client using winit.",
    #[cfg(feature = "udev")]
    "tty-udev : Run anvil as a tty udev client (requires root if without logind).",
    #[cfg(feature = "x11")]
    "x11 : Run anvil as an X11 client.",
];

#[cfg(feature = "profile-with-tracy-mem")]
#[global_allocator]
static GLOBAL: profiling::tracy_client::ProfiledAllocator<std::alloc::System> =
    profiling::tracy_client::ProfiledAllocator::new(std::alloc::System, 10);

fn main() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt()
            .compact()
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt().compact().init();
    }

    #[cfg(feature = "profile-with-tracy")]
    profiling::tracy_client::Client::start();

    profiling::register_thread!("Main Thread");

    #[cfg(feature = "profile-with-puffin")]
    let _server =
        puffin_http::Server::new(&format!("0.0.0.0:{}", puffin_http::DEFAULT_PORT)).unwrap();
    #[cfg(feature = "profile-with-puffin")]
    profiling::puffin::set_scopes_on(true);

    match ::std::env::args().collect::<Vec<String>>().as_slice() {
        #[cfg(feature = "winit")]
        [_, c] if c.as_str() == "winit" => anvil::winit::run_winit(),

        #[cfg(feature = "udev")]
        [_, c] if c.as_str() == "tty-udev" => anvil::udev::run_udev(),

        #[cfg(feature = "x11")]
        [_, c] if c.as_str() == "x11" => anvil::x11::run_x11(),

        _ => {
            #[allow(clippy::disallowed_macros)]
            {
                println!("USAGE: anvil backend");
                println!();
                println!("Possible backends are:");
                for b in POSSIBLE_BACKENDS {
                    println!("\t{}", b);
                }
            }
        }
    }
}
