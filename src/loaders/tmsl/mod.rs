pub mod model;

use std::error::Error;
use std::fs;

use crate::catalog::Catalog;
use model::TmslModel;

pub fn load_tmsl(path: &str) -> Result<Catalog, Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let model: TmslModel = serde_json::from_str(&content)?;
    Ok(Catalog::from_model(&model)?)
}

pub fn save_tmsl(path: &str, catalog: &Catalog) -> Result<(), Box<dyn Error>> {
    let model = catalog.to_tmsl_model();
    let json = serde_json::to_string_pretty(&model)?;
    fs::write(path, json)?;
    Ok(())
}

pub fn load_tmsl_from_op(
    op: &opendal::blocking::Operator,
    path: &str,
) -> Result<Catalog, Box<dyn Error>> {
    let bytes = op.read(path)?;
    let model: TmslModel = serde_json::from_slice(&bytes.to_vec())?;
    Ok(Catalog::from_model(&model)?)
}

pub fn save_tmsl_to_op(
    op: &opendal::blocking::Operator,
    path: &str,
    catalog: &Catalog,
) -> Result<(), Box<dyn Error>> {
    let model = catalog.to_tmsl_model();
    let json = serde_json::to_string_pretty(&model)?;
    op.write(path, json.into_bytes())?;
    Ok(())
}
