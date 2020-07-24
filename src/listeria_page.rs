use crate::*;
use regex::{Regex, RegexBuilder};
use roxmltree;
use serde_json::Value;
use std::collections::HashMap;
use urlencoding;
use wikibase::entity::*;
use wikibase::snak::SnakDataType;
use wikibase::entity_container::EntityContainer;
use wikibase::mediawiki::api::Api;

/* TODO
- check all possible column types
- Sort by P/Q/P
- Sectioning
- Show only preffered values (eg P41 in Q43175)
- Main namespace block
- P/Q/P ?
- coords commonswiki CHECK
- coords dewiki IMPLEMENT region
- actually edit the page

TEMPLATE PARAMETERS
sparql DONE
columns DONE
sort DONE
section IMPLEMENT
min_section IMPLEMENT
autolist IMPLEMENT
language done?
thumb DONE via thumbnail_size()
links IMPLEMENT fully
row_template DONE
header_template DONE
skip_table DONE
wdedit IMPLEMENT
references IMPLEMENT
freq IGNORED
summary DONE
*/


#[derive(Debug, Clone)]
struct TemplateParams {
    sort: SortMode,
    section: Option<String>,
    min_section:u64,
    row_template: Option<String>,
    header_template: Option<String>,
    autolist: Option<String>,
    summary: Option<String>,
    skip_table: bool,
    wdedit: bool,
    references: bool,
    one_row_per_item: bool,
    sort_ascending: bool,
}

impl TemplateParams {
    pub fn new() -> Self {
         Self {
            sort:SortMode::None,
            section: None,
            min_section:2,
            row_template: None,
            header_template: None,
            autolist: None,
            summary: None,
            skip_table: false,
            wdedit: false,
            references: false,
            one_row_per_item: false,
            sort_ascending: true,
         }
    }
}

#[derive(Debug, Clone)]
pub struct ListeriaPage {
    mw_api: Api,
    wd_api: Api,
    wiki: String,
    page: String,
    template_title_start: String,
    language: String,
    template: Option<Template>,
    params: TemplateParams,
    sparql_rows: Vec<HashMap<String, SparqlValue>>,
    sparql_first_variable: Option<String>,
    columns: Vec<Column>,
    entities: EntityContainer,
    links: LinksType,
    local_page_cache: HashMap<String,bool>,
    results: Vec<ResultRow>,
    shadow_files: Vec<String>,
    wikis_to_check_for_shadow_images: Vec<String>,
    data_has_changed: bool,
    simulate: bool,
    simulated_text: Option<String>,
}

impl ListeriaPage {
    pub async fn new(mw_api: &Api, page: String) -> Option<Self> {
        Some(Self {
            mw_api: mw_api.clone(),
            wd_api: Api::new("https://www.wikidata.org/w/api.php")
                .await
                .expect("Could not connect to Wikidata API"),
            wiki: mw_api
                .get_site_info_string("general", "wikiid")
                .expect("No wikiid in site info")
                .to_string(),
            page: page,
            template_title_start: "Wikidata list".to_string(),
            language: mw_api
                .get_site_info_string("general", "lang")
                .ok()?
                .to_string(),
            template: None,
            params: TemplateParams::new(),
            sparql_rows: vec![],
            sparql_first_variable: None,
            columns: vec![],
            entities: EntityContainer::new(),
            links: LinksType::All,
            local_page_cache: HashMap::new(),
            results: vec![],
            shadow_files: vec![],
            wikis_to_check_for_shadow_images: vec!["enwiki".to_string()],
            data_has_changed: false,
            simulate: false,
            simulated_text: None,
        })
    }

    pub fn do_simulate(&mut self,text: Option<String>) {
        self.simulate = true ;
        self.simulated_text = text ;
    }

    pub fn language(&self) -> &String {
        return &self.language;
    }

    pub fn column(&self,column_id:usize) -> Option<&Column> {
        self.columns.get(column_id)
    }

    pub fn get_links_type(&self) -> &LinksType {
        &self.links
    }

    pub async fn run(&mut self) -> Result<(), String> {
        self.load_page().await?;
        self.process_template()?;
        self.run_query().await?;
        self.load_entities().await?;
        self.results = self.get_results()?;
        self.patch_results().await
    }

    pub fn get_local_entity_label(&self, entity_id: &String) -> Option<String> {
        self.entities
            .get_entity(entity_id.to_owned())?
            .label_in_locale(&self.language)
            .map(|s| s.to_string())
    }

    pub fn thumbnail_size(&self) -> u64 {
        let default: u64 = 128;
        let t = match &self.template {
            Some(t) => t,
            None => return default,
        };
        match t.params.get("thumb") {
            Some(s) => s.parse::<u64>().ok().or(Some(default)).unwrap(),
            None => default,
        }
    }

    pub fn external_id_url(&self, prop: &String, id: &String) -> Option<String> {
        let pi = self.entities.get_entity(prop.to_owned())?;
        pi.claims_with_property("P1630")
            .iter()
            .filter_map(|s| {
                let data_value = s.main_snak().data_value().to_owned()?;
                match data_value.value() {
                    wikibase::Value::StringValue(s) => 
                        Some(
                        s.to_owned()
                            .replace("$1", &urlencoding::decode(&id).ok()?.to_string()),
                    ),
                    _ => None,
                }
            })
            .next()
    }

    pub fn get_entity<S: Into<String>>(&self, entity_id: S) -> Option<wikibase::Entity> {
        self.entities.get_entity(entity_id)
    }

    pub fn get_location_template(&self, lat: f64, lon: f64) -> String {
        // Hardcoded special cases!!1!
        if self.wiki == "wikidatawiki" {
            return format!("{}/{}",lat,lon);
        }
        if self.wiki == "commonswiki" {
            return format!("{{Inline coordinates|{}|{}|display=inline}}}}",lat,lon);
        }
        if self.wiki == "dewiki" {
            // TODO get region for item
            let q = "" ;
            let region = "" ;
            return format!("{{{{Coordinate|text=DMS|NS={}|EW={}|name={}|simple=y|type=landmark|region={}}}}}",lat,lon,q,region);
        }
        format!("{{{{Coord|{}|{}|display=inline}}}}", lat, lon) // en; default
    }

    pub fn get_row_template(&self) -> &Option<String> {
        &self.params.row_template
    }

    async fn cache_local_page_exists(&mut self,page:String) {
        let params: HashMap<String, String> = vec![
            ("action", "query"),
            ("prop", ""),
            ("titles", page.as_str()),
        ]
        .iter()
        .map(|x| (x.0.to_string(), x.1.to_string()))
        .collect();

        let result = match self
            .mw_api
            .get_query_api_json(&params)
            .await {
                Ok(r) => r,
                Err(_e) => return
            };
            
        let page_exists = match result["query"]["pages"].as_object() {
            Some(obj) => {
                obj
                .iter()
                .filter(|(_k,v)|v["missing"].as_str().is_some())
                .count()==0 // No "missing"=existing
            }
            None => false // Dunno
        };
        self.local_page_cache.insert(page,page_exists);
    }

    pub fn local_page_exists(&self,page:&str) -> bool {
        *self.local_page_cache.get(&page.to_string()).unwrap_or(&false)
    }

    async fn load_page(&mut self) -> Result<(), String> {
        let text = self.load_page_as("parsetree").await?.to_owned();
        let doc = roxmltree::Document::parse(&text).unwrap();
        doc.root()
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "template")
            .for_each(|node| {
                if self.template.is_some() {
                    return;
                }
                match Template::new_from_xml(&node) {
                    Some(t) => {
                        if t.title == self.template_title_start {
                            self.template = Some(t);
                        }
                    }
                    None => {}
                }
            });
        Ok(())
    }

    fn process_template(&mut self) -> Result<(), String> {
        let template = match &self.template {
            Some(t) => t.clone(),
            None => {
                return Err(format!(
                    "No template '{}' found",
                    &self.template_title_start
                ))
            }
        };

        match template.params.get("columns") {
            Some(columns) => {
                columns.split(",").for_each(|part| {
                    let s = part.clone().to_string();
                    self.columns.push(Column::new(&s));
                });
            }
            None => self.columns.push(Column::new(&"item".to_string())),
        }

        self.params = TemplateParams {
            sort: SortMode::new(template.params.get("sort")),
            section: template.params.get("section").map(|s|s.trim().to_uppercase()),
            min_section: template
                            .params
                            .get("min_section")
                            .map(|s|
                                s.parse::<u64>().ok().or(Some(2)).unwrap_or(2)
                                )
                            .unwrap_or(2),
            row_template: template.params.get("row_template").map(|s|s.trim().to_string()),
            header_template: template.params.get("header_template").map(|s|s.trim().to_string()),
            autolist: template.params.get("autolist").map(|s|s.trim().to_uppercase()) ,
            summary: template.params.get("summary").map(|s|s.trim().to_uppercase()) ,
            skip_table: template.params.get("skip_table").is_some(),
            one_row_per_item: template.params.get("one_row_per_item").map(|s|s.trim().to_uppercase())!=Some("NO".to_string()),
            wdedit: template.params.get("wdedit").map(|s|s.trim().to_uppercase())==Some("YES".to_string()),
            references: template.params.get("references").map(|s|s.trim().to_uppercase())==Some("ALL".to_string()),
            sort_ascending: template.params.get("sort_order").map(|s|s.trim().to_uppercase())!=Some("DESC".to_string()),
        } ;

        match template.params.get("language") {
            Some(l) =>  self.language = l.to_lowercase(),
            None => {}
        }

        match template.params.get("links") {
            Some(s) =>  self.links = LinksType::new_from_string(s.to_string()),
            None => {}
        }

        Ok(())
    }

    async fn run_query(&mut self) -> Result<(), String> {
        let t = match &self.template {
            Some(t) => t,
            None => return Err(format!("No template found")),
        };
        let sparql = match t.params.get("sparql") {
            Some(s) => s,
            None => return Err(format!("No `sparql` parameter in {:?}", &t)),
        };

        let j = match self.wd_api.sparql_query(sparql).await {
            Ok(j) => j,
            Err(e) => return Err(format!("{:?}", &e)),
        };
        self.parse_sparql(j)
    }

    fn parse_sparql(&mut self, j: Value) -> Result<(), String> {
        self.sparql_rows.clear();
        self.sparql_first_variable = None;

        // TODO force first_var to be "item" for backwards compatability?
        // Or check if it is, and fail if not?
        let first_var = match j["head"]["vars"].as_array() {
            Some(a) => match a.get(0) {
                Some(v) => v.as_str().ok_or("Can't parse first variable")?.to_string(),
                None => return Err(format!("Bad SPARQL head.vars")),
            },
            None => return Err(format!("Bad SPARQL head.vars")),
        };
        self.sparql_first_variable = Some(first_var.clone());

        let bindings = j["results"]["bindings"]
            .as_array()
            .ok_or("Broken SPARQL results.bindings")?;
        for b in bindings.iter() {
            let mut row: HashMap<String, SparqlValue> = HashMap::new();
            for (k, v) in b.as_object().unwrap().iter() {
                match SparqlValue::new_from_json(&v) {
                    Some(v2) => row.insert(k.to_owned(), v2),
                    None => return Err(format!("Can't parse SPARQL value: {} => {:?}", &k, &v)),
                };
            }
            if row.is_empty() {
                continue;
            }
            self.sparql_rows.push(row);
        }
        Ok(())
    }

    async fn load_entities(&mut self) -> Result<(), String> {
        // Any columns that require entities to be loaded?
        // TODO also force if self.links is redlinks etc.
        if self
            .columns
            .iter()
            .filter(|c| match c.obj {
                ColumnType::Number => false,
                ColumnType::Item => false,
                ColumnType::Field(_) => false,
                _ => true,
            })
            .count()
            == 0
        {
            return Ok(());
        }

        let ids = self.get_ids_from_sparql_rows()?;
        if ids.is_empty() {
            return Err(format!("No items to show"));
        }
        match self.entities.load_entities(&self.wd_api, &ids).await {
            Ok(_) => {}
            Err(e) => return Err(format!("Error loading entities: {:?}", &e)),
        }

        self.label_columns();

        Ok(())
    }

    fn label_columns(&mut self) {
        self.columns = self
            .columns
            .iter()
            .map(|c| {
                let mut c = c.clone();
                c.generate_label(self);
                c
            })
            .collect();
    }

    fn get_ids_from_sparql_rows(&self) -> Result<Vec<String>, String> {
        let varname = self.get_var_name()?;

        // Rows
        let ids_tmp: Vec<String> = self
            .sparql_rows
            .iter()
            .filter_map(|row| match row.get(varname) {
                Some(SparqlValue::Entity(id)) => Some(id.to_string()),
                _ => None,
            })
            .collect();

        let mut ids: Vec<String> = vec![] ;
        ids_tmp.iter().for_each(|id|{
            if !ids.contains(id) {
                ids.push(id.to_string());
            }
        });

        // Can't sort/dedup, need to preserve original order
        //ids.sort();
        //ids.dedup();

        // Column headers
        self.columns.iter().for_each(|c| match &c.obj {
            ColumnType::Property(prop) => {
                ids.push(prop.to_owned());
            }
            ColumnType::PropertyQualifier((prop, qual)) => {
                ids.push(prop.to_owned());
                ids.push(qual.to_owned());
            }
            ColumnType::PropertyQualifierValue((prop1, qual, prop2)) => {
                ids.push(prop1.to_owned());
                ids.push(qual.to_owned());
                ids.push(prop2.to_owned());
            }
            _ => {}
        });

        Ok(ids)
    }

    fn get_parts_p_p(&self,statement:&wikibase::statement::Statement,property:&String) -> Vec<ResultCellPart> {
        statement
            .qualifiers()
            .iter()
            .filter(|snak|*snak.property()==*property)
            .map(|snak|{
                let ret = ResultCellPart::SnakList (
                    vec![
                        ResultCellPart::from_snak(statement.main_snak()),
                        ResultCellPart::from_snak(snak)
                    ]
                ) ;
                /*
                ret.iter().for_each(|rcp|{
                    match rcp {

                    }self.lazy_load_item()
                })
                */
                ret
            })
            .collect()
    }

    fn get_result_cell(
        &self,
        entity_id: &String,
        sparql_rows: &Vec<&HashMap<String, SparqlValue>>,
        col: &Column,
    ) -> ResultCell {
        let mut ret = ResultCell::new();

        let entity = self.entities.get_entity(entity_id.to_owned());
        match &col.obj {
            ColumnType::Item => {
                ret.parts
                    .push(ResultCellPart::Entity((entity_id.to_owned(), true)));
            }
            ColumnType::Description => match entity {
                Some(e) => match e.description_in_locale(self.language.as_str()) {
                    Some(s) => {
                        ret.parts.push(ResultCellPart::Text(s.to_string()));
                    }
                    None => {}
                },
                None => {}
            },
            ColumnType::Field(varname) => {
                for row in sparql_rows.iter() {
                    match row.get(varname) {
                        Some(x) => {
                            ret.parts.push(ResultCellPart::from_sparql_value(x));
                        }
                        None => {}
                    }
                }
            }
            ColumnType::Property(property) => match entity {
                Some(e) => {
                    e.claims_with_property(property.to_owned())
                        .iter()
                        .for_each(|statement| {
                            ret.parts
                                .push(ResultCellPart::from_snak(statement.main_snak()));
                        });
                }
                None => {}
            },
            ColumnType::PropertyQualifier((p1, p2)) => match entity {
                Some(e) => {
                    e.claims_with_property(p1.to_owned())
                        .iter()
                        .for_each(|statement| {
                            self.get_parts_p_p(statement,p2)
                                .iter()
                                .for_each(|part|ret.parts.push(part.to_owned()));
                        });
                }
                None => {}
            },
            ColumnType::LabelLang(language) => match entity {
                Some(e) => {
                    match e.label_in_locale(language) {
                        Some(s) => {
                            ret.parts.push(ResultCellPart::Text(s.to_string()));
                        }
                        None => match e.label_in_locale(&self.language) {
                            // Fallback
                            Some(s) => {
                                ret.parts.push(ResultCellPart::Text(s.to_string()));
                            }
                            None => {} // No label available
                        },
                    }
                }
                None => {}
            },
            ColumnType::Label => match entity {
                Some(e) => {
                    let label = match e.label_in_locale(&self.language) {
                        Some(s) => s.to_string(),
                        None => entity_id.to_string(),
                    };
                    let local_page = match e.sitelinks() {
                        Some(sl) => sl
                            .iter()
                            .filter(|s| *s.site() == self.wiki)
                            .map(|s| s.title().to_string())
                            .next(),
                        None => None,
                    };
                    match local_page {
                        Some(page) => {
                            ret.parts.push(ResultCellPart::LocalLink((page, label)));
                        }
                        None => {
                            ret.parts
                                .push(ResultCellPart::Entity((entity_id.to_string(), true)));
                        }
                    }
                }
                None => {}
            },
            ColumnType::Unknown => {} // Ignore
            ColumnType::Number => {
                ret.parts.push(ResultCellPart::Number);
            }
            _ => {} /*
                    // TODO
                    PropertyQualifierValue((String, String, String)),
                    */
        }

        ret
    }

    fn get_result_row(
        &self,
        entity_id: &String,
        sparql_rows: &Vec<&HashMap<String, SparqlValue>>,
    ) -> Option<ResultRow> {
        match self.links {
            LinksType::Local => {
                if !self.entities.has_entity(entity_id.to_owned()) {
                    return None;
                }
            }
            _ => {}
        }

        let mut row = ResultRow::new(entity_id);
        row.cells = self
            .columns
            .iter()
            .map(|col| self.get_result_cell(entity_id, sparql_rows, col))
            .collect();
        Some(row)
    }

    fn get_var_name(&self) -> Result<&String, String> {
        match &self.sparql_first_variable {
            Some(v) => Ok(v),
            None => return Err(format!("load_entities: sparql_first_variable is None")),
        }
    }

    fn get_results(&mut self) -> Result<Vec<ResultRow>, String> {
        let varname = self.get_var_name()?;
        Ok(match self.params.one_row_per_item {
            true => self
                .get_ids_from_sparql_rows()?
                .iter()
                .filter_map(|id| {
                    let sparql_rows: Vec<&HashMap<String, SparqlValue>> = self
                        .sparql_rows
                        .iter()
                        .filter(|row| match row.get(varname) {
                            Some(SparqlValue::Entity(v)) => v == id,
                            _ => false,
                        })
                        .collect();
                    if !sparql_rows.is_empty() {
                        self.get_result_row(id, &sparql_rows)
                    } else {
                        None
                    }
                })
                .collect(),
            false => self
                .sparql_rows
                .iter()
                .filter_map(|row| match row.get(varname) {
                    Some(SparqlValue::Entity(id)) => self.get_result_row(id, &vec![&row]),
                    _ => None,
                })
                .collect(),
        })
    }

    fn entity_to_local_link(&self, item: &String) -> Option<ResultCellPart> {
        let entity = match self.entities.get_entity(item.to_owned()) {
            Some(e) => e,
            None => return None,
        };
        let page = match entity.sitelinks() {
            Some(sl) => sl
                .iter()
                .filter(|s| *s.site() == self.wiki)
                .map(|s| s.title().to_string())
                .next(),
            None => None,
        }?;
        let label = self.get_local_entity_label(item).unwrap_or(page.clone());
        Some(ResultCellPart::LocalLink((page, label)))
    }

    fn localize_item_links_in_parts(&self,parts:&Vec<ResultCellPart>) -> Vec<ResultCellPart> {
        parts.iter()
        .map(|part| match part {
            ResultCellPart::Entity((item, true)) => {
                match self.entity_to_local_link(&item) {
                    Some(ll) => ll,
                    None => part.to_owned(),
                }
            }
            ResultCellPart::SnakList(v) => {
                ResultCellPart::SnakList(self.localize_item_links_in_parts(v))
            }
            _ => part.to_owned(),
        })
        .collect()
    }

    fn patch_items_to_local_links(&mut self) -> Result<(), String> {
        // Try to change items to local link
        // TODO mutate in place; fn in ResultRow. This is pathetic.
        self.results = self
            .results
            .iter()
            .map(|row| ResultRow {
                entity_id: row.entity_id.to_owned(),
                cells: row
                    .cells
                    .iter()
                    .map(|cell| ResultCell {
                        parts: self.localize_item_links_in_parts(&cell.parts),
                    })
                    .collect(),
                section:row.section,
                sortkey: row.sortkey.to_owned()
            })
            .collect();
        Ok(())
    }

    async fn load_items(&mut self, mut entities_to_load:Vec<String>) -> Result<(), String> {
        entities_to_load.sort() ;
        entities_to_load.dedup();
        match self.entities.load_entities(&self.wd_api, &entities_to_load).await {
            Ok(_) => {}
            Err(e) => return Err(format!("Error loading entities: {:?}", &e)),
        }
        Ok(())
    }

    async fn gather_and_load_items_sort(&mut self) -> Result<(), String> {
        match &self.params.sort {
            SortMode::Property(prop) => {
                let mut entities_to_load = vec![];
                for row in self.results.iter() {
                    match self.entities.get_entity(row.entity_id.to_owned()) {
                        Some(entity) => {
                            entity
                                .claims()
                                .iter()
                                .filter(|statement|statement.property()==prop)
                                .map(|statement|statement.main_snak())
                                .filter(|snak|*snak.datatype()==SnakDataType::WikibaseItem)
                                .filter_map(|snak|snak.data_value().to_owned())
                                .map(|datavalue|datavalue.value().to_owned())
                                .filter_map(|value|match value {
                                    wikibase::value::Value::Entity(v) => Some(v.id().to_owned()),
                                    _ => None
                                })
                                .for_each(|id|entities_to_load.push(id.to_string()));
                        }
                        None => {}
                    }
                }
                self.load_items(entities_to_load).await?;
            }
            _ => {}
        }
        Ok(())
    }

    fn gather_entities_and_external_properties(&self,parts:&Vec<ResultCellPart>) -> Vec<String> {
        let mut entities_to_load = vec![];
        for part in parts {
            match part {
                ResultCellPart::Entity((item, true)) => {
                    entities_to_load.push(item.to_owned());
                }
                ResultCellPart::ExternalId((property, _id)) => {
                    entities_to_load.push(property.to_owned());
                }
                ResultCellPart::SnakList(v) => {
                    self.gather_entities_and_external_properties(&v)
                        .iter()
                        .for_each(|entity_id|entities_to_load.push(entity_id.to_string()))
                }
                _ => {}
            }
        }
        entities_to_load
    }

    async fn gather_and_load_items(&mut self) -> Result<(), String> {
        // Gather items to load
        let mut entities_to_load : Vec<String> = vec![];
        for row in self.results.iter() {
            for cell in &row.cells {
                self.gather_entities_and_external_properties(&cell.parts)
                    .iter()
                    .for_each(|entity_id|entities_to_load.push(entity_id.to_string()));
            }
        }
        match &self.params.sort {
            SortMode::Property(prop) => {
                entities_to_load.push(prop.to_string());
            }
            _ => {}
        }
        self.load_items(entities_to_load).await?;
        self.gather_and_load_items_sort().await?;
        Ok(())
    }

    async fn patch_remove_shadow_files(&mut self) -> Result<(), String> {
        if !self.wikis_to_check_for_shadow_images.contains(&self.wiki) {
            return Ok(())
        }
        let mut files_to_check = vec![] ;
        for row in self.results.iter() {
            for cell in &row.cells {
                for part in &cell.parts {
                    match part {
                        ResultCellPart::File(file) => {
                            files_to_check.push(file);
                        }
                        _ => {}
                    }
                }
            }
        }
        files_to_check.sort_unstable();
        files_to_check.dedup();

        self.shadow_files.clear();

        // TODO better async
        for filename in files_to_check {
            let prefixed_filename = format!("{}:{}",self.local_file_namespace_prefix(),&filename) ;
            let params: HashMap<String, String> =
                vec![("action", "query"), ("titles", prefixed_filename.as_str()),("prop","imageinfo")]
                    .iter()
                    .map(|x| (x.0.to_string(), x.1.to_string()))
                    .collect();

            let j = match self.mw_api.get_query_api_json(&params).await {
                Ok(j) => j,
                Err(_e) => json!({})
            };

            let mut could_be_local = false ;
            match j["query"]["pages"].as_object() {
                Some(results) => {
                    results.iter().for_each(|(_k, o)|{
                        match o["imagerepository"].as_str() {
                            Some("shared") => {},
                            _ => { could_be_local = true ; }
                        }
                    })
                }
                None => { could_be_local = true ; }
            };

            if could_be_local {
                self.shadow_files.push(filename.to_string());
            }
        }

        self.shadow_files.sort();

        // Remove shadow files from data table
        // TODO this is less than ideal in terms of pretty code...
        let shadow_files = &self.shadow_files;
        self.results.iter_mut().for_each(|row|{
            row.cells.iter_mut().for_each(|cell|{
                cell.parts = cell.parts.iter().filter(|part|{
                    match part {
                        ResultCellPart::File(file) => !shadow_files.contains(file),
                        _ => true
                    }
                })
                .cloned()
                .collect();
            });
        });

        Ok(())
    }

    fn patch_redlinks_only(&mut self) -> Result<(), String> {
        if *self.get_links_type() != LinksType::RedOnly {
            return Ok(())
        }

        // Remove all rows with existing local page  
        // TODO better iter things
        self.results = self.results
            .iter()
            .filter(|row|{
                let entity = self.entities.get_entity(row.entity_id.to_owned()).unwrap();
                match entity.sitelinks() {
                    Some(sl) => {
                        sl
                        .iter()
                        .filter(|s| *s.site() == self.wiki)
                        .count() == 0
                    }
                    None => true, // No sitelinks, keep
                }
            })
            .cloned()
            .collect();
        Ok(())
    }

    async fn patch_redlinks(&mut self) -> Result<(), String> {
        if *self.get_links_type() != LinksType::RedOnly && *self.get_links_type() != LinksType::Red {
            return Ok(())
        }

        // Cache if local pages exist
        let mut ids = vec![] ;
        self.results.iter().for_each(|row|{
            row.cells.iter().for_each(|cell|{
                cell.parts
                    .iter()
                    .for_each(|part|{
                    match part {
                        ResultCellPart::Entity((id, _try_localize)) => {
                            ids.push(id);
                        }
                        _ => {}
                    }
                })
            });
        });

        ids.sort();
        ids.dedup();
        let mut labels = vec![] ;
        for id in ids {
            match self.get_entity(id.to_owned()) {
                Some(e) => match e.label_in_locale(self.language()) {
                    Some(l) => {
                        labels.push(l.to_string());
                    }
                    None => {}
                }
                None => {}
            }
        }

        labels.sort();
        labels.dedup();
        for label in labels {
            self.cache_local_page_exists(label).await;
        }

        Ok(())
    }

    fn get_datatype_for_property(&self,prop:&String) -> SnakDataType {
        match self.get_entity(prop) {
            Some(entity) => {
                match entity {
                    Entity::Property(p) => {
                        match p.datatype() {
                            Some(t) => t.to_owned(),
                            None => SnakDataType::String
                        }
                    }
                    _ => SnakDataType::String
                }
            }
            None => SnakDataType::String
        }
    }

    fn patch_sort_results(&mut self) -> Result<(), String> {
        let sortkeys : Vec<String> ;
        let mut datatype = SnakDataType::String ; // Default
        match &self.params.sort {
            SortMode::Label => {
                sortkeys = self.results
                    .iter()
                    .map(|row|row.get_sortkey_label(&self))
                    .collect();
            }
            SortMode::FamilyName => {
                sortkeys = self.results
                    .iter()
                    .map(|row|row.get_sortkey_family_name(&self))
                    .collect();
            }
            SortMode::Property(prop) => {
                datatype = self.get_datatype_for_property(prop);
                sortkeys = self.results
                    .iter()
                    .map(|row|row.get_sortkey_prop(&prop,&self,&datatype))
                    .collect();
            }
            _ => return Ok(())
        }

        // Apply sortkeys
        if self.results.len() != sortkeys.len() { // Paranoia
            return Err(format!("patch_sort_results: sortkeys length mismatch"));
        }
        self.results
            .iter_mut()
            .enumerate()
            .for_each(|(rownum, row)|row.set_sortkey(sortkeys[rownum].to_owned())) ;

        self.results.sort_by(|a, b| a.compare_to(b,&datatype));
        if !self.params.sort_ascending {
            self.results.reverse()
        }

        //self.results.iter().for_each(|row|println!("{}: {}",&row.entity_id,&row.sortkey));
        Ok(())
    }
    
    async fn patch_results(&mut self) -> Result<(), String> {
        self.gather_and_load_items().await? ;
        self.patch_redlinks_only()?;
        self.patch_items_to_local_links()?;
        self.patch_redlinks().await?;
        self.patch_remove_shadow_files().await?;
        self.patch_sort_results()?;
        Ok(())
    }

    fn get_section_ids(&self) -> Vec<usize> {
        let mut ret : Vec<usize> = self
            .results
            .iter()
            .map(|row|{row.section})
            .collect();
        ret.sort_unstable();
        ret.dedup();
        ret
    }

    fn as_wikitext_table_header(&self) -> String {
        let mut wt = String::new() ;
        match &self.params.header_template {
            Some(t) => {
                wt += "{{" ;
                wt +=  &t ;
                wt += "}}\n" ;
            }
            None => {
                if !self.params.skip_table {
                    wt += "{| class='wikitable sortable' style='width:100%'\n" ;
                    self.columns.iter().enumerate().for_each(|(_colnum,col)| {
                        wt += "! " ;
                        wt += &col.label ;
                        wt += "\n" ;
                    });
                }
            }
        }
        wt
    }

    fn as_wikitext_section(&self,section_id:usize) -> String {
        let mut wt = String::new() ;

        // TODO: section header

        wt += &self.as_wikitext_table_header() ;

        if self.params.row_template.is_none() && !self.params.skip_table {
            if !self.results.is_empty() {
                wt += "|-\n";
            }
        }

        // Rows
        let rows = self
            .results
            .iter()
            .filter(|row|row.section==section_id)
            .enumerate()
            .map(|(rownum, row)| row.as_wikitext(&self, rownum))
            .collect::<Vec<String>>() ;
        if self.params.skip_table {
            wt += &rows.join("\n");
        } else {
            wt += &rows.join("\n|-\n");
        }

        // End
        if !self.params.skip_table {
            wt += "\n|}" ;
        }

        wt
    }

    pub fn normalize_page_title(&self,s: &String) -> String {
        // TODO use page to find out about first character capitalization on the current wiki
        if s.len() < 2 {
            return s.to_string();
        }
        let (first_letter, the_rest) = s.split_at(1);
        return first_letter.to_uppercase() + the_rest;
    }

    pub fn local_file_namespace_prefix(&self) -> String {
        "File".to_string() // TODO
    }

    pub fn as_wikitext(&self) -> Result<String,String> {
        let section_ids = self.get_section_ids() ;
        // TODO section headers
        let mut wt : String = section_ids
            .iter()
            .map(|section_id|self.as_wikitext_section(*section_id))
            .collect() ;

        if !self.shadow_files.is_empty() {
            wt += "\n----\nThe following local image(s) are not shown in the above list, because they shadow a Commons image of the same name, and might be non-free:";
            for file in &self.shadow_files {
                wt += format!("\n# [[:{}:{}|]]",self.local_file_namespace_prefix(),file).as_str();
            }
        }

        match self.params.summary.as_ref().map(|s|s.as_str()) {
            Some("ITEMNUMBER") => {
                wt += format!("\n----\n&sum; {} items.",self.results.len()).as_str();
            }
            _ => {}
        }

        Ok(wt)
    }

    pub fn as_tabbed_data(&self) -> Result<Value, String> {
        let mut ret = json!({"license": "CC0-1.0","description": {"en":"Listeria output"},"sources":"https://github.com/magnusmanske/listeria_rs","schema":{"fields":[{ "name": "section", "type": "number", "title": { self.language.to_owned(): "Section"}}]},"data":[]});
        self.columns.iter().enumerate().for_each(|(colnum,col)| {
            ret["schema"]["fields"]
                .as_array_mut()
                .unwrap() // OK, this must exist
                .push(json!({"name":"col_".to_string()+&colnum.to_string(),"type":"string","title":{self.language.to_owned():col.label}}))
        });
        ret["data"] = self
            .results
            .iter()
            .enumerate()
            .map(|(rownum, row)| row.as_tabbed_data(&self, rownum))
            .collect();
        Ok(ret)
    }

    pub fn tabbed_data_page_name(&self) -> Option<String> {
        let ret = "Data:Listeria/".to_string() + &self.wiki + "/" + &self.page + ".tab";
        if ret.len() > 250 {
            return None; // Page title too long
        }
        Some(ret)
    }

    pub async fn write_tabbed_data(
        &mut self,
        tabbed_data_json: Value,
        commons_api: &mut Api,
    ) -> Result<(), String> {
        let data_page = self
            .tabbed_data_page_name()
            .ok_or("Data page name too long")?;
        let text = ::serde_json::to_string(&tabbed_data_json).unwrap();
        let params: HashMap<String, String> = vec![
            ("action", "edit"),
            ("title", data_page.as_str()),
            ("summary", "Listeria test"),
            ("text", text.as_str()),
            ("minor", "true"),
            ("recreate", "true"),
            ("token", commons_api.get_edit_token().await.unwrap().as_str()),
        ]
        .iter()
        .map(|x| (x.0.to_string(), x.1.to_string()))
        .collect();
        // No need to check if this is the same as the existing data; MW API will return OK but not actually edit
        let _result = match commons_api.post_query_api_json_mut(&params).await {
            Ok(r) => r,
            Err(e) => return Err(format!("{:?}", e)),
        };
        // TODO check ["edit"]["result"] == "Success"
        // TODO set data_has_changed is result is not "same as before"
        self.data_has_changed = true; // Just to make sure to update including page
        Ok(())
    }

    async fn load_page_as(&self, mode: &str) -> Result<String, String> {
        let mut params: HashMap<String, String> = vec![
            ("action", "parse"),
            ("prop", mode),
//            ("page", self.page.as_str()),
        ]
        .iter()
        .map(|x| (x.0.to_string(), x.1.to_string()))
        .collect();

        match &self.simulated_text {
            Some(t) => {
                params.insert("title".to_string(),self.page.clone());
                params.insert("text".to_string(),t.to_string());
            }
            None => {
                params.insert("page".to_string(),self.page.clone());
            }
        }

        let result = self
            .mw_api
            .get_query_api_json(&params)
            .await
            .expect("Loading page failed");
        match result["parse"][mode]["*"].as_str() {
            Some(ret) => Ok(ret.to_string()),
            None => return Err(format!("No parse tree for {}", &self.page)),
        }
    }

    fn separate_start_template(&self, blob: &String) -> Option<(String, String)> {
        let mut split_at: Option<usize> = None;
        let mut curly_count: i32 = 0;
        blob.char_indices().for_each(|(pos, c)| {
            match c {
                '{' => {
                    curly_count += 1;
                }
                '}' => {
                    curly_count -= 1;
                }
                _ => {}
            }
            if curly_count == 0 && split_at.is_none() {
                split_at = Some(pos + 1);
            }
        });
        match split_at {
            Some(pos) => {
                let mut template = blob.clone();
                let rest = template.split_off(pos);
                Some((template, rest))
            }
            None => None,
        }
    }

    pub async fn update_source_page(&self) -> Result<(), String> {
        let wikitext = self.load_page_as("wikitext").await?;

        // TODO use local template name

        // Start/end template
        let pattern1 =
            r#"^(.*?)(\{\{[Ww]ikidata[ _]list\b.+)(\{\{[Ww]ikidata[ _]list[ _]end\}\})(.*)"#;

        // No end template
        let pattern2 = r#"^(.*?)(\{\{[Ww]ikidata[ _]list\b.+)"#;

        let re_wikitext1: Regex = RegexBuilder::new(pattern1)
            .multi_line(true)
            .dot_matches_new_line(true)
            .build()
            .unwrap();
        let re_wikitext2: Regex = RegexBuilder::new(pattern2)
            .multi_line(true)
            .dot_matches_new_line(true)
            .build()
            .unwrap();

        let (before, blob, end_template, after) = match re_wikitext1.captures(&wikitext) {
            Some(caps) => (
                caps.get(1).unwrap().as_str(),
                caps.get(2).unwrap().as_str(),
                caps.get(3).unwrap().as_str(),
                caps.get(4).unwrap().as_str(),
            ),
            None => match re_wikitext2.captures(&wikitext) {
                Some(caps) => (
                    caps.get(1).unwrap().as_str(),
                    caps.get(2).unwrap().as_str(),
                    "",
                    "",
                ),
                None => return Err(format!("No template/end template found")),
            },
        };

        let (start_template, rest) = match self.separate_start_template(&blob.to_string()) {
            Some(parts) => parts,
            None => return Err(format!("Can't split start template")),
        };

        let append = if end_template.is_empty() {
            rest.to_string()
        } else {
            after.to_string()
        };

        // Remove tabbed data marker
        let start_template = Regex::new(r"\|\s*tabbed_data[^\|\}]*")
            .unwrap()
            .replace(&start_template, "");

        // Add tabbed data marker
        let start_template = start_template[0..start_template.len() - 2]
            .trim()
            .to_string()
            + "\n|tabbed_data=1}}";

        // Create new wikitext
        let new_wikitext = before.to_owned() + &start_template + "\n" + append.trim();

        // Compare to old wikitext
        if wikitext == new_wikitext {
            // All is as it should be
            if self.data_has_changed {
                self.purge_page().await?;
            }
            return Ok(());
        }

        // TODO edit page

        Ok(())
    }

    async fn purge_page(&self) -> Result<(), String> {
        if self.simulate {
            println!("SIMULATING: purging [[{}]] on {}", &self.page,self.wiki);
            return Ok(())
        }
        let params: HashMap<String, String> =
            vec![("action", "purge"), ("titles", self.page.as_str())]
                .iter()
                .map(|x| (x.0.to_string(), x.1.to_string()))
                .collect();

        match self.mw_api.get_query_api_json(&params).await {
            Ok(_r) => Ok(()),
            Err(e) => return Err(format!("{:?}", e)),
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
        let rows = text.split("\n");
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
                value += "\n";
                value += row ;
            }
        }
        if !key.is_empty() {
            data.insert(key,value.trim().to_string());
        }
        data
    }

    async fn check_fixture_file(path:PathBuf) {
        let data = read_fixture_from_file ( path ) ;
        let mw_api = wikibase::mediawiki::api::Api::new(&data["API"]).await.unwrap();
        let mut page = ListeriaPage::new(&mw_api, data["PAGETITLE"].clone()).await.unwrap();
        page.do_simulate(data.get("WIKITEXT").map(|s|s.to_string()));
        page.run().await.unwrap();
        let wt = page.as_wikitext().unwrap().trim().to_string();
        if data.contains_key("EXPECTED") {
            //println!("Checking EXPECTED");
            //println!("{}",&wt);
            assert_eq!(wt,data["EXPECTED"]);
        }
        if data.contains_key("EXPECTED_PART") {
            //println!("Checking EXPECTED_PART");
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

    /*
    // I want all of it, Molari, ALL OF IT!
    #[tokio::test]
    async fn fixtures() {
        let paths = fs::read_dir("./test_data").unwrap();
        for path in paths {
            let path = path.unwrap();
            println!("Fixture {}",path.path().display());
            check_fixture_file(path.path()).await;
        }
    }
    */
}
