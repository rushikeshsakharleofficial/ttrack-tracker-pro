use anyhow::Result;
use ttrack_pro::config::Config;
use ttrack_pro::daemon;

fn main() -> Result<()> {
    let cfg = Config::load();
    daemon::run(cfg)
}
