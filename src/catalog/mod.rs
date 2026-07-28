use crate::loaders::tmsl::model::{
    Column as TmslColumn, DaxDataType, Measure as TmslMeasure, Relationship, Table as TmslTable,
    TmslModel,
};
pub use crate::loaders::tmsl::model::{ColumnDataCategory, SummarizeBy, TableDataCategory};
use chrono::{DateTime, Utc};
use polars::prelude::{DataType, TimeUnit};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ColumnMeta {
    pub table: String,
    pub column: String,
    pub dtype: DataType,
    pub summarize_by: SummarizeBy,
    pub is_hidden: bool,
    pub format_string: Option<String>,
    pub display_folder: Option<String>,
    pub data_category: Option<ColumnDataCategory>,
    pub description: Option<String>,
    pub sort_by_column: Option<String>,
    pub is_key: bool,
    pub is_nullable: bool,
    pub is_unique: bool,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub columns: HashMap<(String, String), ColumnMeta>,
    pub measures: HashMap<String, String>,
    pub measure_tables: HashMap<String, String>,
    pub relationships: Vec<Relationship>,
    pub data_sources: HashMap<String, String>,
    pub table_is_hidden: HashMap<String, bool>,
    pub table_data_category: HashMap<String, TableDataCategory>,
    pub table_description: HashMap<String, String>,
    pub measure_is_hidden: HashMap<String, bool>,
    pub measure_format_string: HashMap<String, String>,
    pub measure_display_folder: HashMap<String, String>,
    pub measure_description: HashMap<String, String>,
    pub model_name: Option<String>,
    pub culture: Option<String>,
    pub collation: Option<String>,
    pub compatibility_level: Option<u32>,
    pub default_mode: Option<String>,
    pub table_order: Vec<String>,
    pub column_order: HashMap<String, Vec<String>>,
    pub measure_order: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub last_schema_update: DateTime<Utc>,
    pub last_refreshed: DateTime<Utc>,
}

macro_rules! move_key {
    ($map:expr, $old:expr, $new:expr) => {
        if let Some(v) = $map.remove($old) {
            $map.insert($new.to_string(), v);
        }
    };
}

impl Catalog {
    pub fn from_model(model: &TmslModel) -> Result<Self, String> {
        let mut columns = HashMap::new();
        let mut measures = HashMap::new();
        let mut measure_tables = HashMap::new();
        let mut data_sources = HashMap::new();
        let mut table_is_hidden = HashMap::new();
        let mut table_data_category = HashMap::new();
        let mut table_description = HashMap::new();
        let mut measure_is_hidden = HashMap::new();
        let mut measure_format_string = HashMap::new();
        let mut measure_display_folder = HashMap::new();
        let mut measure_description = HashMap::new();

        let mut table_order: Vec<String> = Vec::new();
        let mut column_order: HashMap<String, Vec<String>> = HashMap::new();
        let mut measure_order: Vec<String> = Vec::new();

        for table in &model.tables {
            let table_name = &table.name;

            table_order.push(table_name.clone());
            column_order
                .entry(table_name.clone())
                .or_default()
                .extend(table.columns.iter().map(|c| c.name.clone()));

            data_sources.insert(table_name.clone(), table.data_source.clone());
            table_is_hidden.insert(table_name.clone(), table.is_hidden);
            if let Some(dc) = &table.data_category {
                table_data_category.insert(table_name.clone(), dc.clone());
            }
            if let Some(d) = &table.description {
                table_description.insert(table_name.clone(), d.clone());
            }

            for col in &table.columns {
                let dtype = DataType::try_from(&col.data_type)
                    .map_err(|e| format!("Column '{}': {e}", col.name))?;
                columns.insert(
                    (table_name.clone(), col.name.clone()),
                    ColumnMeta {
                        table: table_name.clone(),
                        column: col.name.clone(),
                        dtype,
                        summarize_by: col.summarize_by.clone(),
                        is_hidden: col.is_hidden,
                        format_string: col.format_string.clone(),
                        display_folder: col.display_folder.clone(),
                        data_category: col.data_category.clone(),
                        description: col.description.clone(),
                        sort_by_column: col.sort_by_column.clone(),
                        is_key: col.is_key,
                        is_nullable: col.is_nullable,
                        is_unique: col.is_unique,
                    },
                );
            }
            for col in &table.columns {
                if let Some(ref sort_by) = col.sort_by_column {
                    if !columns.contains_key(&(table_name.clone(), sort_by.clone())) {
                        return Err(format!(
                            "Column '{}.{}': sortByColumn '{}' does not exist in table '{}'",
                            table_name, col.name, sort_by, table_name
                        ));
                    }
                }
            }

            for m in &table.measures {
                // First occurrence wins for duplicate measure names.
                if measures
                    .insert(m.name.clone(), m.expression.clone())
                    .is_none()
                {
                    measure_order.push(m.name.clone());
                }
                measure_tables.insert(m.name.clone(), table_name.clone());
                measure_is_hidden.insert(m.name.clone(), m.is_hidden);
                if let Some(fs) = &m.format_string {
                    measure_format_string.insert(m.name.clone(), fs.clone());
                }
                if let Some(df) = &m.display_folder {
                    measure_display_folder.insert(m.name.clone(), df.clone());
                }
                if let Some(d) = &m.description {
                    measure_description.insert(m.name.clone(), d.clone());
                }
            }
        }

        for rel in &model.relationships {
            let from_key = (rel.from_table.clone(), rel.from_column.clone());
            let to_key = (rel.to_table.clone(), rel.to_column.clone());

            let from_meta = columns.get(&from_key).ok_or_else(|| {
                format!(
                    "Relationship '{}': from-column '{}.{}' not declared in model",
                    rel.name, rel.from_table, rel.from_column
                )
            })?;

            let to_meta = columns.get(&to_key).ok_or_else(|| {
                format!(
                    "Relationship '{}': to-column '{}.{}' not declared in model",
                    rel.name, rel.to_table, rel.to_column
                )
            })?;

            if from_meta.dtype != to_meta.dtype {
                return Err(format!(
                    "Relationship '{}': join column type mismatch — \
                     '{}.{}' is {:?} but '{}.{}' is {:?}",
                    rel.name,
                    rel.from_table,
                    rel.from_column,
                    from_meta.dtype,
                    rel.to_table,
                    rel.to_column,
                    to_meta.dtype,
                ));
            }
        }

        let now = Utc::now();
        Ok(Self {
            columns,
            measures,
            measure_tables,
            relationships: model.relationships.clone(),
            data_sources,
            table_is_hidden,
            table_data_category,
            table_description,
            measure_is_hidden,
            measure_format_string,
            measure_display_folder,
            measure_description,
            model_name: model.name.clone(),
            culture: model.culture.clone(),
            collation: model.collation.clone(),
            compatibility_level: model.compatibility_level,
            default_mode: model.default_mode.clone(),
            table_order,
            column_order,
            measure_order,
            created_at: now,
            last_schema_update: now,
            last_refreshed: now,
        })
    }

    fn touch(&mut self) {
        self.last_schema_update = Utc::now();
    }

    // Model ────────────────────────────────────────────────────────────────

    pub fn rename_model(&mut self, new_name: &str) {
        self.model_name = Some(new_name.to_string());
        self.touch();
    }

    pub fn set_culture(&mut self, value: String) {
        self.culture = Some(value);
        self.touch();
    }

    pub fn set_collation(&mut self, value: String) {
        self.collation = Some(value);
        self.touch();
    }

    pub fn set_default_mode(&mut self, value: String) {
        self.default_mode = Some(value);
        self.touch();
    }

    // Tables ───────────────────────────────────────────────────────────────

    fn has_table(&self, name: &str) -> bool {
        self.table_is_hidden.contains_key(name)
    }

    pub fn create_table(&mut self, name: &str) -> Result<(), String> {
        if self.has_table(name) {
            return Err(format!("Table '{name}' already exists"));
        }
        self.table_order.push(name.to_string());
        self.column_order.insert(name.to_string(), Vec::new());
        self.table_is_hidden.insert(name.to_string(), false);
        self.data_sources.insert(name.to_string(), String::new());
        self.touch();
        Ok(())
    }

    pub fn delete_table(&mut self, table: &str) -> Result<(), String> {
        if !self.has_table(table) {
            return Err(format!("Table '{table}' does not exist"));
        }
        if let Some(cols) = self.column_order.remove(table) {
            for col in cols {
                self.columns.remove(&(table.to_string(), col));
            }
        }
        self.table_order.retain(|t| t != table);
        self.table_is_hidden.remove(table);
        self.table_data_category.remove(table);
        self.table_description.remove(table);
        self.data_sources.remove(table);
        let measures_to_remove: Vec<String> = self
            .measure_tables
            .iter()
            .filter(|(_, t)| t.as_str() == table)
            .map(|(m, _)| m.clone())
            .collect();
        for m in measures_to_remove {
            self.measures.remove(&m);
            self.measure_tables.remove(&m);
            self.measure_is_hidden.remove(&m);
            self.measure_format_string.remove(&m);
            self.measure_display_folder.remove(&m);
            self.measure_description.remove(&m);
            self.measure_order.retain(|n| n != &m);
        }
        self.relationships
            .retain(|r| r.from_table != table && r.to_table != table);
        self.touch();
        Ok(())
    }

    pub fn rename_table(&mut self, table: &str, new_name: &str) -> Result<(), String> {
        if !self.has_table(table) {
            return Err(format!("Table '{table}' does not exist"));
        }
        if self.has_table(new_name) {
            return Err(format!("Table '{new_name}' already exists"));
        }
        for t in &mut self.table_order {
            if t == table {
                *t = new_name.to_string();
            }
        }
        if let Some(cols) = self.column_order.remove(table) {
            self.column_order.insert(new_name.to_string(), cols);
        }
        let old_keys: Vec<String> = self
            .columns
            .keys()
            .filter(|(t, _)| t == table)
            .map(|(_, c)| c.clone())
            .collect();
        for col in old_keys {
            if let Some(mut meta) = self.columns.remove(&(table.to_string(), col.clone())) {
                meta.table = new_name.to_string();
                self.columns.insert((new_name.to_string(), col), meta);
            }
        }
        move_key!(self.table_is_hidden, table, new_name);
        move_key!(self.table_data_category, table, new_name);
        move_key!(self.table_description, table, new_name);
        move_key!(self.data_sources, table, new_name);
        for v in self.measure_tables.values_mut() {
            if v == table {
                *v = new_name.to_string();
            }
        }
        for r in &mut self.relationships {
            if r.from_table == table {
                r.from_table = new_name.to_string();
            }
            if r.to_table == table {
                r.to_table = new_name.to_string();
            }
        }
        self.touch();
        Ok(())
    }

    pub fn set_table_is_hidden(&mut self, table: &str, value: bool) -> Result<(), String> {
        self.check_table(table)?;
        self.table_is_hidden.insert(table.to_string(), value);
        self.touch();
        Ok(())
    }

    pub fn set_table_description(
        &mut self,
        table: &str,
        value: Option<String>,
    ) -> Result<(), String> {
        self.check_table(table)?;
        match value {
            Some(v) => self.table_description.insert(table.to_string(), v),
            None => self.table_description.remove(table),
        };
        self.touch();
        Ok(())
    }

    pub fn set_table_data_category(
        &mut self,
        table: &str,
        value: Option<TableDataCategory>,
    ) -> Result<(), String> {
        self.check_table(table)?;
        match value {
            Some(v) => self.table_data_category.insert(table.to_string(), v),
            None => self.table_data_category.remove(table),
        };
        self.touch();
        Ok(())
    }

    fn check_table(&self, table: &str) -> Result<(), String> {
        if self.has_table(table) {
            Ok(())
        } else {
            Err(format!("Table '{table}' does not exist"))
        }
    }

    // Columns ──────────────────────────────────────────────────────────────

    pub fn add_column(&mut self, table: &str, meta: ColumnMeta) -> Result<(), String> {
        if !self.has_table(table) {
            return Err(format!("Table '{table}' does not exist"));
        }
        let key = (table.to_string(), meta.column.clone());
        if self.columns.contains_key(&key) {
            return Err(format!("Column '{}.{}' already exists", table, meta.column));
        }
        self.column_order
            .entry(table.to_string())
            .or_default()
            .push(meta.column.clone());
        self.columns.insert(key, meta);
        self.touch();
        Ok(())
    }

    pub fn delete_column(&mut self, table: &str, column: &str) -> Result<(), String> {
        let key = (table.to_string(), column.to_string());
        if !self.columns.contains_key(&key) {
            return Err(format!("Column '{table}.{column}' does not exist"));
        }
        self.columns.remove(&key);
        if let Some(cols) = self.column_order.get_mut(table) {
            cols.retain(|c| c != column);
        }
        self.touch();
        Ok(())
    }

    pub fn rename_column(
        &mut self,
        table: &str,
        column: &str,
        new_name: &str,
    ) -> Result<(), String> {
        let key = (table.to_string(), column.to_string());
        let new_key = (table.to_string(), new_name.to_string());
        if !self.columns.contains_key(&key) {
            return Err(format!("Column '{table}.{column}' does not exist"));
        }
        if self.columns.contains_key(&new_key) {
            return Err(format!("Column '{table}.{new_name}' already exists"));
        }
        let mut meta = self
            .columns
            .remove(&key)
            .expect("key existence confirmed by contains_key check above");
        meta.column = new_name.to_string();
        self.columns.insert(new_key, meta);
        if let Some(cols) = self.column_order.get_mut(table) {
            for c in cols.iter_mut() {
                if c == column {
                    *c = new_name.to_string();
                }
            }
        }
        self.touch();
        Ok(())
    }

    pub fn change_column_type(
        &mut self,
        table: &str,
        column: &str,
        dtype: DataType,
    ) -> Result<(), String> {
        let meta = self
            .columns
            .get_mut(&(table.to_string(), column.to_string()))
            .ok_or_else(|| format!("Column '{table}.{column}' does not exist"))?;
        meta.dtype = dtype;
        self.touch();
        Ok(())
    }

    pub fn set_column_is_hidden(
        &mut self,
        table: &str,
        column: &str,
        value: bool,
    ) -> Result<(), String> {
        self.col_mut(table, column)?.is_hidden = value;
        self.touch();
        Ok(())
    }

    pub fn set_column_description(
        &mut self,
        table: &str,
        column: &str,
        value: Option<String>,
    ) -> Result<(), String> {
        self.col_mut(table, column)?.description = value;
        self.touch();
        Ok(())
    }

    pub fn set_column_format_string(
        &mut self,
        table: &str,
        column: &str,
        value: Option<String>,
    ) -> Result<(), String> {
        self.col_mut(table, column)?.format_string = value;
        self.touch();
        Ok(())
    }

    pub fn set_column_display_folder(
        &mut self,
        table: &str,
        column: &str,
        value: Option<String>,
    ) -> Result<(), String> {
        self.col_mut(table, column)?.display_folder = value;
        self.touch();
        Ok(())
    }

    pub fn set_column_data_category(
        &mut self,
        table: &str,
        column: &str,
        value: Option<ColumnDataCategory>,
    ) -> Result<(), String> {
        self.col_mut(table, column)?.data_category = value;
        self.touch();
        Ok(())
    }

    pub fn set_column_sort_by(
        &mut self,
        table: &str,
        column: &str,
        value: Option<String>,
    ) -> Result<(), String> {
        if let Some(target) = &value {
            if !self
                .columns
                .contains_key(&(table.to_string(), target.clone()))
            {
                return Err(format!(
                    "sortByColumn target '{table}.{target}' does not exist"
                ));
            }
        }
        self.col_mut(table, column)?.sort_by_column = value;
        self.touch();
        Ok(())
    }

    pub fn set_column_summarize_by(
        &mut self,
        table: &str,
        column: &str,
        value: SummarizeBy,
    ) -> Result<(), String> {
        self.col_mut(table, column)?.summarize_by = value;
        self.touch();
        Ok(())
    }

    fn col_mut(&mut self, table: &str, column: &str) -> Result<&mut ColumnMeta, String> {
        self.columns
            .get_mut(&(table.to_string(), column.to_string()))
            .ok_or_else(|| format!("Column '{table}.{column}' does not exist"))
    }

    // Measures ─────────────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn create_measure(
        &mut self,
        table: &str,
        name: &str,
        expression: &str,
        format_string: Option<String>,
        is_hidden: bool,
        description: Option<String>,
        display_folder: Option<String>,
    ) -> Result<(), String> {
        if !self.has_table(table) {
            return Err(format!("Table '{table}' does not exist"));
        }
        if self.measures.contains_key(name) {
            return Err(format!("Measure '{name}' already exists"));
        }
        self.measures
            .insert(name.to_string(), expression.to_string());
        self.measure_tables
            .insert(name.to_string(), table.to_string());
        self.measure_order.push(name.to_string());
        self.measure_is_hidden.insert(name.to_string(), is_hidden);
        if let Some(fs) = format_string {
            self.measure_format_string.insert(name.to_string(), fs);
        }
        if let Some(d) = description {
            self.measure_description.insert(name.to_string(), d);
        }
        if let Some(df) = display_folder {
            self.measure_display_folder.insert(name.to_string(), df);
        }
        self.touch();
        Ok(())
    }

    pub fn delete_measure(&mut self, name: &str) -> Result<(), String> {
        if !self.measures.contains_key(name) {
            return Err(format!("Measure '{name}' does not exist"));
        }
        self.measures.remove(name);
        self.measure_tables.remove(name);
        self.measure_is_hidden.remove(name);
        self.measure_format_string.remove(name);
        self.measure_display_folder.remove(name);
        self.measure_description.remove(name);
        self.measure_order.retain(|m| m != name);
        self.touch();
        Ok(())
    }

    pub fn rename_measure(&mut self, name: &str, new_name: &str) -> Result<(), String> {
        if !self.measures.contains_key(name) {
            return Err(format!("Measure '{name}' does not exist"));
        }
        if self.measures.contains_key(new_name) {
            return Err(format!("Measure '{new_name}' already exists"));
        }
        move_key!(self.measures, name, new_name);
        move_key!(self.measure_tables, name, new_name);
        move_key!(self.measure_is_hidden, name, new_name);
        move_key!(self.measure_format_string, name, new_name);
        move_key!(self.measure_display_folder, name, new_name);
        move_key!(self.measure_description, name, new_name);
        for m in &mut self.measure_order {
            if m == name {
                *m = new_name.to_string();
            }
        }
        self.touch();
        Ok(())
    }

    pub fn update_measure_expression(
        &mut self,
        name: &str,
        expression: &str,
    ) -> Result<(), String> {
        let expr = self
            .measures
            .get_mut(name)
            .ok_or_else(|| format!("Measure '{name}' does not exist"))?;
        *expr = expression.to_string();
        self.touch();
        Ok(())
    }

    pub fn set_measure_format_string(
        &mut self,
        name: &str,
        format_string: Option<String>,
    ) -> Result<(), String> {
        if !self.measures.contains_key(name) {
            return Err(format!("Measure '{name}' does not exist"));
        }
        match format_string {
            Some(v) => self.measure_format_string.insert(name.to_string(), v),
            None => self.measure_format_string.remove(name),
        };
        self.touch();
        Ok(())
    }

    // Relationships ────────────────────────────────────────────────────────

    pub fn create_relationship(&mut self, rel: Relationship) -> Result<(), String> {
        if self.relationships.iter().any(|r| r.name == rel.name) {
            return Err(format!("Relationship '{}' already exists", rel.name));
        }
        if !self
            .columns
            .contains_key(&(rel.from_table.clone(), rel.from_column.clone()))
        {
            return Err(format!(
                "Column '{}.{}' does not exist",
                rel.from_table, rel.from_column
            ));
        }
        if !self
            .columns
            .contains_key(&(rel.to_table.clone(), rel.to_column.clone()))
        {
            return Err(format!(
                "Column '{}.{}' does not exist",
                rel.to_table, rel.to_column
            ));
        }
        self.relationships.push(rel);
        self.touch();
        Ok(())
    }

    pub fn delete_relationship(&mut self, name: &str) -> Result<(), String> {
        let idx = self
            .relationships
            .iter()
            .position(|r| r.name == name)
            .ok_or_else(|| format!("Relationship '{name}' does not exist"))?;
        self.relationships.remove(idx);
        self.touch();
        Ok(())
    }

    pub fn update_relationship(
        &mut self,
        name: &str,
        is_active: Option<bool>,
        bidirectional: Option<bool>,
    ) -> Result<(), String> {
        let rel = self
            .relationships
            .iter_mut()
            .find(|r| r.name == name)
            .ok_or_else(|| format!("Relationship '{name}' does not exist"))?;
        if let Some(v) = is_active {
            rel.active = v;
        }
        if let Some(v) = bidirectional {
            rel.bidirectional = v;
        }
        self.touch();
        Ok(())
    }

    pub fn to_tmsl_model(&self) -> TmslModel {
        let tables = self
            .table_order
            .iter()
            .map(|table_name| {
                let col_names = self
                    .column_order
                    .get(table_name)
                    .cloned()
                    .unwrap_or_default();
                let columns = col_names
                    .iter()
                    .filter_map(|col_name| {
                        let meta = self.columns.get(&(table_name.clone(), col_name.clone()))?;
                        Some(TmslColumn {
                            name: col_name.clone(),
                            data_type: DaxDataType::from(&meta.dtype),
                            summarize_by: meta.summarize_by.clone(),
                            is_hidden: meta.is_hidden,
                            format_string: meta.format_string.clone(),
                            display_folder: meta.display_folder.clone(),
                            data_category: meta.data_category.clone(),
                            description: meta.description.clone(),
                            sort_by_column: meta.sort_by_column.clone(),
                            is_key: meta.is_key,
                            is_nullable: meta.is_nullable,
                            is_unique: meta.is_unique,
                        })
                    })
                    .collect();

                let measures = self
                    .measure_order
                    .iter()
                    .filter_map(|m_name| {
                        if self.measure_tables.get(m_name)? != table_name {
                            return None;
                        }
                        Some(TmslMeasure {
                            name: m_name.clone(),
                            expression: self.measures.get(m_name)?.clone(),
                            is_hidden: self.measure_is_hidden.get(m_name).copied().unwrap_or(false),
                            format_string: self.measure_format_string.get(m_name).cloned(),
                            display_folder: self.measure_display_folder.get(m_name).cloned(),
                            description: self.measure_description.get(m_name).cloned(),
                        })
                    })
                    .collect();

                TmslTable {
                    name: table_name.clone(),
                    data_source: self
                        .data_sources
                        .get(table_name)
                        .cloned()
                        .unwrap_or_default(),
                    columns,
                    measures,
                    is_hidden: self
                        .table_is_hidden
                        .get(table_name)
                        .copied()
                        .unwrap_or(false),
                    data_category: self.table_data_category.get(table_name).cloned(),
                    description: self.table_description.get(table_name).cloned(),
                }
            })
            .collect();

        TmslModel {
            name: self.model_name.clone(),
            tables,
            relationships: self.relationships.clone(),
            culture: self.culture.clone(),
            collation: self.collation.clone(),
            compatibility_level: self.compatibility_level,
            default_mode: self.default_mode.clone(),
        }
    }
}

impl TryFrom<&DaxDataType> for DataType {
    type Error = String;

    fn try_from(dt: &DaxDataType) -> Result<DataType, String> {
        match dt {
            DaxDataType::String => Ok(DataType::String),
            DaxDataType::Int64 => Ok(DataType::Int64),
            DaxDataType::Double => Ok(DataType::Float64),
            DaxDataType::Decimal => Ok(DataType::Float64),
            DaxDataType::Boolean => Ok(DataType::Boolean),
            DaxDataType::DateTime => Ok(DataType::Datetime(TimeUnit::Milliseconds, None)),
            DaxDataType::Binary => Ok(DataType::Binary),
            other => Err(format!("unsupported DAX type {other:?}")),
        }
    }
}

impl From<&DataType> for DaxDataType {
    fn from(dt: &DataType) -> DaxDataType {
        match dt {
            DataType::String => DaxDataType::String,
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::Int128
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::UInt128 => DaxDataType::Int64,
            DataType::Float16 | DataType::Float32 | DataType::Float64 => DaxDataType::Double,
            DataType::Boolean => DaxDataType::Boolean,
            DataType::Date | DataType::Datetime(_, _) => DaxDataType::DateTime,
            DataType::Binary | DataType::BinaryOffset => DaxDataType::Binary,
            DataType::Duration(_)
            | DataType::Time
            | DataType::List(_)
            | DataType::Null
            | DataType::Unknown(_)
            | DataType::Categorical(_, _)
            | DataType::Enum(_, _)
            | DataType::Struct(_) => DaxDataType::Unknown,
        }
    }
}
