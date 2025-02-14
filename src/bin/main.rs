extern crate config;
extern crate serde_json;

use anyhow::{Result,anyhow};
use config::{Config, File};
use listeria::configuration::Configuration;
use listeria::listeria_page::ListeriaPage;
use std::env;
use std::sync::Arc;
use tokio::sync::RwLock;

async fn update_page(_settings: &Config, page_title: &str, api_url: &str) -> Result<String> {
    let config = Arc::new(Configuration::new_from_file("config.json").await.unwrap());
    let mut mw_api = wikibase::mediawiki::api::Api::new(api_url).await?;
    // let token = settings.get_string("user.token").expect("No oauth2 user.token");
    // mw_api.set_oauth2(&token);
    mw_api.set_oauth2(config.oauth2_token());

    let mw_api = Arc::new(RwLock::new(mw_api));
    let mut page = ListeriaPage::new(config, mw_api, page_title.into()).await?;
    page.run().await.map_err(|e|anyhow!("{e:?}"))?;

    Ok(match page.update_source_page().await.map_err(|e|anyhow!("{e:?}"))? {
        true => format!("{page_title} edited"),
        false => format!("{page_title} not edited"),
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let ini_file = "listeria.ini";

    let settings = Config::builder()
        .add_source(File::new(ini_file, config::FileFormat::Ini))
        .build()
        .unwrap_or_else(|_| panic!("INI file '{}' can't be opened", ini_file));

    let args: Vec<String> = env::args().collect();
    let wiki_server = args
        .get(1)
        .ok_or_else(|| anyhow!("No wiki server argument"))?;
    let page = args.get(2).ok_or_else(|| anyhow!("No page argument"))?;

    let wiki_api = format!("https://{}/w/api.php", &wiki_server);
    let message = match update_page(&settings, &page, &wiki_api).await {
        Ok(m) => format!("OK: {}", m),
        Err(e) => format!("ERROR: {}", e),
    };
    println!("{}", message);
    Ok(())
}
