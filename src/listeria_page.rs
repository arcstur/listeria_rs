use crate::*;
use std::sync::Arc;
use std::collections::HashMap;
use wikibase::mediawiki::api::Api;

/* TODO
- Sort by P/P, P/Q/P DOES NOT WORK IN LISTERIA-PHP
- coords commonswiki CHECK
- coords dewiki IMPLEMENT region
- api parameter to override default
- actually edit the page

TEMPLATE PARAMETERS
links IMPLEMENT fully
wdedit IMPLEMENT
references IMPLEMENT
freq IGNORED => bot manager

min_section DONE
section DONE
sparql DONE
columns DONE
sort DONE
language done?
thumb DONE via thumbnail_size()
row_template DONE
header_template DONE
skip_table DONE
summary DONE
*/

#[derive(Debug, Clone)]
pub struct ListeriaPage {
    pub page_params: PageParams,
    template: Option<Template>,
    results: Vec<ResultRow>,
    data_has_changed: bool,
    lists:Vec<ListeriaList>,
}

impl ListeriaPage {
    pub async fn new(config: Arc<Configuration>, mw_api: Arc<Api>, page: String) -> Result<Self,String> {
        let page_params = PageParams::new(config, mw_api, page).await? ;
        Ok(Self {
            page_params,
            template: None,
            results: vec![],
            data_has_changed: false,
            lists:vec![],
        })
    }

    pub fn do_simulate(&mut self,text: Option<String>, sparql_results:Option<String>) {
        self.page_params.simulate = true ;
        self.page_params.simulated_text = text ;
        self.page_params.simulated_sparql_results = sparql_results ;
    }

    pub fn language(&self) -> &String {
        &self.page_params.language
    }

    pub fn check_namespace(&self) -> Result<(),String> {
        let title = wikibase::mediawiki::title::Title::new_from_full(&self.page_params.page,&self.page_params.mw_api);
        if self.page_params.config.can_edit_namespace(&self.page_params.wiki,title.namespace_id()) {
            Ok(())
        } else {
            Err(format!("Namespace {} not allowed for edit on {}",title.namespace_id(),&self.page_params.wiki))
        }
    }

    pub async fn run(&mut self) -> Result<(), String> {
        self.check_namespace()?;
        self.lists.clear();
        let templates = self.load_page().await?;
        for template in templates {
            let mut list = ListeriaList::new(template.clone(),self.page_params.clone()) ;
            self.template = Some(template.clone());
            list.process_template()?;
            list.run_query().await?;
            list.load_entities().await?;
            list.generate_results().await?;
            list.patch_results().await?;
            self.lists.push(list);
        }
        Ok(())
    }


    async fn load_page(&mut self) -> Result<Vec<Template>, String> {
        let text = self.load_page_as("parsetree").await?;
        let doc = roxmltree::Document::parse(&text).unwrap();
        let template_start = self.page_params.get_local_template_title_start()? ;
        let ret = doc.root()
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "template")
            .filter_map(|node| {
                match Template::new_from_xml(&node) {
                    Some(t) => {
                        // HARDCODED EN AS FALLBACK
                        if t.title == template_start || t.title == "Wikidata list" {
                            Some(t)
                        } else {
                            None
                        }
                    }
                    None => None
                }
            })
            .collect::<Vec<Template>>();
        Ok(ret)
    }

    pub async fn load_page_as(&self, mode: &str) -> Result<String, String> {
        let mut params: HashMap<String, String> = vec![
            ("action", "parse"),
            ("prop", mode),
        ]
        .iter()
        .map(|x| (x.0.to_string(), x.1.to_string()))
        .collect();

        match &self.page_params.simulated_text {
            Some(t) => {
                params.insert("title".to_string(),self.page_params.page.clone());
                params.insert("text".to_string(),t.to_string());
            }
            None => {
                params.insert("page".to_string(),self.page_params.page.clone());
            }
        }

        let result = self
            .page_params
            .mw_api
            .get_query_api_json(&params)
            .await
            .expect("Loading page failed");
        match result["parse"][mode]["*"].as_str() {
            Some(ret) => Ok(ret.to_string()),
            None => Err(format!("No parse tree for {}", &self.page_params.page)),
        }
    }

    pub fn as_wikitext(&self) -> Result<Vec<String>,String> {
        let mut ret : Vec<String> = vec!();
        for list in &self.lists {
            let mut renderer = RendererWikitext::new();
            ret.push(renderer.render(&list)?);
        }
        Ok(ret)
    }

    pub fn lists(&self) -> &Vec<ListeriaList> {
        &self.lists
    }


    pub async fn update_source_page(&self,renderer: &impl Renderer) -> Result<(), String> {
        let wikitext = self.load_page_as("wikitext").await?;

        let new_wikitext = renderer.get_new_wikitext(&wikitext,self)? ;

        match new_wikitext {
            Some(_wikitext) => {}
            None => {
                if self.data_has_changed {
                    self.purge_page().await?;
                }    
            }
        }
        // TODO edit page

        Ok(())
    }

    async fn purge_page(&self) -> Result<(), String> {
        if self.page_params.simulate {
            println!("SIMULATING: purging [[{}]] on {}", &self.page_params.page,self.page_params.wiki);
            return Ok(())
        }
        let params: HashMap<String, String> =
            vec![("action", "purge"), ("titles", self.page_params.page.as_str())]
                .iter()
                .map(|x| (x.0.to_string(), x.1.to_string()))
                .collect();

        match self.page_params.mw_api.get_query_api_json(&params).await {
            Ok(_r) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs ;
    use std::path::PathBuf;
    use crate::* ;

    fn read_fixture_from_file(path:PathBuf) -> HashMap<String,String> {
        let text = fs::read_to_string(path).unwrap();
        let rows = text.split('\n');
        let mut key = String::new();
        let mut value = String::new();
        let mut data : HashMap<String,String> = HashMap::new() ;
        for row in rows {
            if row.starts_with("$$$$") {
                if !key.is_empty() {
                    data.insert(key,value.trim().to_string()) ;
                }
                value.clear() ;
                key = row.strip_prefix("$$$$").unwrap().trim().to_string().to_uppercase();
            } else {
                value += &format!("\n{}",row);
            }
        }
        if !key.is_empty() {
            data.insert(key,value.trim().to_string());
        }
        data
    }

    async fn check_fixture_file(path:PathBuf) {
        //println!("{:?}",&path);
        let data = read_fixture_from_file ( path.clone() ) ;
        let mw_api = wikibase::mediawiki::api::Api::new(&data["API"]).await.unwrap();
        let mw_api = Arc::new(mw_api);

        let file = File::open("config.json").unwrap();
        let reader = BufReader::new(file);
        let mut j : Value = serde_json::from_reader(reader).unwrap();
        j["namespace_blocks"] = json!({}); // Allow all namespaces, everywhere
        if path.to_str().unwrap() == "test_data/shadow_images.fixture" { // HACKISH
            j["prefer_preferred"] = json!(false) ;
        }
        let config = Arc::new(Configuration::new_from_json(j).unwrap());
        let mut page = ListeriaPage::new(config,mw_api, data["PAGETITLE"].clone()).await.unwrap();
        page.do_simulate(data.get("WIKITEXT").map(|s|s.to_string()),data.get("SPARQL_RESULTS").map(|s|s.to_string()));
        page.run().await.unwrap();
        let wt = page.as_wikitext().unwrap();
        let wt = wt.join("\n\n----\n\n");
        let wt = wt.trim().to_string();
        if data.contains_key("EXPECTED") {
            assert_eq!(wt,data["EXPECTED"]);
        }
        if data.contains_key("EXPECTED_PART") {
            assert!(wt.contains(&data["EXPECTED_PART"]));
        }
    }

    #[tokio::test]
    async fn shadow_images() {
        check_fixture_file(PathBuf::from("test_data/shadow_images.fixture")).await;
    }

    #[tokio::test]
    async fn summary_itemnumber() {
        check_fixture_file(PathBuf::from("test_data/summary_itemnumber.fixture")).await;
    }

    #[tokio::test]
    async fn header_template() {
        check_fixture_file(PathBuf::from("test_data/header_template.fixture")).await;
    }

    #[tokio::test]
    async fn header_row_template() {
        check_fixture_file(PathBuf::from("test_data/header_row_template.fixture")).await;
    }

    #[tokio::test]
    async fn links_all() {
        check_fixture_file(PathBuf::from("test_data/links_all.fixture")).await;
    }

    #[tokio::test]
    async fn links_red() {
        check_fixture_file(PathBuf::from("test_data/links_red.fixture")).await;
    }

    #[tokio::test]
    async fn links_red_only() {
        check_fixture_file(PathBuf::from("test_data/links_red_only.fixture")).await;
    }

    #[tokio::test]
    async fn links_text() {
        check_fixture_file(PathBuf::from("test_data/links_text.fixture")).await;
    }

    #[tokio::test]
    async fn links_local() {
        check_fixture_file(PathBuf::from("test_data/links_local.fixture")).await;
    }

    #[tokio::test]
    async fn links_reasonator() {
        check_fixture_file(PathBuf::from("test_data/links_reasonator.fixture")).await;
    }

    #[tokio::test]
    async fn date_extid_quantity() {
        check_fixture_file(PathBuf::from("test_data/date_extid_quantity.fixture")).await;
    }

    #[tokio::test]
    async fn coordinates() {
        check_fixture_file(PathBuf::from("test_data/coordinates.fixture")).await;
    }

    #[tokio::test]
    async fn sort_label() {
        check_fixture_file(PathBuf::from("test_data/sort_label.fixture")).await;
    }

    #[tokio::test]
    async fn sort_prop_item() {
        check_fixture_file(PathBuf::from("test_data/sort_prop_item.fixture")).await;
    }

    #[tokio::test]
    async fn sort_prop_time() {
        check_fixture_file(PathBuf::from("test_data/sort_prop_time.fixture")).await;
    }

    #[tokio::test]
    async fn sort_prop_string() {
        check_fixture_file(PathBuf::from("test_data/sort_prop_string.fixture")).await;
    }

    #[tokio::test]
    async fn sort_prop_quantity() {
        check_fixture_file(PathBuf::from("test_data/sort_prop_quantity.fixture")).await;
    }

    #[tokio::test]
    async fn sort_prop_monolingual() {
        check_fixture_file(PathBuf::from("test_data/sort_prop_monolingual.fixture")).await;
    }

    #[tokio::test]
    async fn sort_reverse() {
        check_fixture_file(PathBuf::from("test_data/sort_reverse.fixture")).await;
    }

    #[tokio::test]
    async fn sort_family_name() {
        check_fixture_file(PathBuf::from("test_data/sort_family_name.fixture")).await;
    }

    #[tokio::test]
    async fn columns() {
        check_fixture_file(PathBuf::from("test_data/columns.fixture")).await;
    }

    #[tokio::test]
    async fn p_p() {
        check_fixture_file(PathBuf::from("test_data/p_p.fixture")).await;
    }

    #[tokio::test]
    async fn p_q_p() {
        check_fixture_file(PathBuf::from("test_data/p_q_p.fixture")).await;
    }

    #[tokio::test]
    async fn sections() {
        check_fixture_file(PathBuf::from("test_data/sections.fixture")).await;
    }

    #[tokio::test]
    async fn preferred_rank() {
        check_fixture_file(PathBuf::from("test_data/preferred_rank.fixture")).await;
    }

    #[tokio::test]
    async fn multiple_lists() {
        check_fixture_file(PathBuf::from("test_data/multiple_lists.fixture")).await;
    }

    #[tokio::test]
    async fn autodesc() {
        check_fixture_file(PathBuf::from("test_data/autodesc.fixture")).await;
    }

    #[tokio::test]
    async fn dewiki() {
        check_fixture_file(PathBuf::from("test_data/dewiki.fixture")).await;
    }

    #[tokio::test]
    async fn edit_wikitext() {
        let data = read_fixture_from_file ( PathBuf::from("test_data/edit_wikitext.fixture") ) ;
        let mw_api = wikibase::mediawiki::api::Api::new("https://en.wikipedia.org/w/api.php").await.unwrap();
        let mw_api = Arc::new(mw_api);
        let config = Arc::new(Configuration::new_from_file("config.json").unwrap());
        let mut page = ListeriaPage::new(config,mw_api, "User:Magnus Manske/listeria test5".to_string()).await.unwrap();
        page.do_simulate(data.get("WIKITEXT").map(|s|s.to_string()),data.get("SPARQL_RESULTS").map(|s|s.to_string()));
        page.run().await.unwrap();
        let wikitext = page.load_page_as("wikitext").await.expect("FAILED load page as wikitext");
        let renderer = RendererWikitext::new();
        let wt = renderer.get_new_wikitext(&wikitext,&page).expect("FAILED get_new_wikitext").expect("new_wikitext not Some()");
        let wt = wt.trim().to_string();
        assert_eq!(wt,data["EXPECTED"]);
    }

    /*
    // I want all of it, Molari, ALL OF IT!
    #[tokio::test]
    async fn all_fixtures() {
        let paths = fs::read_dir("./test_data").unwrap();
        for path in paths {
            let path = path.unwrap();
            println!("Fixture {}",path.path().display());
            check_fixture_file(path.path()).await;
        }
    }
    */
}
