use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::post,
    Router,
};

use http_body_util::{BodyExt, Limited};
use std::sync::Arc;
use uuid::Uuid;

const MAX_XMLA_BODY_BYTES: usize = 64 * 1024 * 1024;

mod codec;
mod handlers;
mod soap;

use crate::mdx::ast::ConditionValue;
use crate::mdx::{mdx_to_dax, parse_mdx, FromClause, QueryShape};
use codec::XmlaCodec;

use crate::server::{config::ServerConfig, ServerProvider};

#[derive(Clone)]
struct AppState {
    provider: Arc<dyn ServerProvider>,
    config: Arc<ServerConfig>,
}

pub fn routes(provider: Arc<dyn ServerProvider>, config: Arc<ServerConfig>) -> Router {
    Router::new()
        .route("/xmla", post(xmla_handler))
        .with_state(AppState { provider, config })
}

async fn xmla_handler(State(state): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    let provider = &state.provider;
    let config = &state.config;
    let t_request = std::time::Instant::now();
    let body_bytes = match Limited::new(body, MAX_XMLA_BODY_BYTES).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            tracing::warn!(error = %e, "failed to read request body (oversized or malformed)");
            return Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(Body::from(format!("failed to read request body: {e}")))
                .expect("status + unadorned body cannot produce an invalid response");
        }
    };

    let codec = XmlaCodec::from_headers(&headers);
    let body_str = codec.decode_request(&body_bytes);

    let soap_action = headers
        .get("soapaction")
        .or_else(|| headers.get("SOAPAction"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let negotiation_flags = headers
        .get("x-ms-xmlacaps-negotiation-flags")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("(absent)");
    tracing::debug!(soap_action, negotiation_flags, "incoming XMLA request");

    let req_headers: String = headers
        .iter()
        .map(|(k, v)| format!("  {}: {}", k, v.to_str().unwrap_or("<binary>")))
        .collect::<Vec<_>>()
        .join("\n");
    tracing::debug!(headers = %req_headers, "request headers");
    tracing::trace!(body = %pretty_xml(&body_str), "request body");

    let envelope = match soap::Envelope::parse(&body_str) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse SOAP envelope");
            let xml = handlers::empty_ok(None).0;
            let (bytes, ct) = codec.encode_response(&xml);
            return Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", ct)
                .body(Body::from(bytes))
                .expect("Content-Type is always one of codec's fixed &'static str values");
        }
    };

    let incoming_session_id = headers
        .get("x-ms-xmlasession-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| envelope.session_id().map(|s| s.to_string()));

    let session_id = incoming_session_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let response_session: Option<String> = if envelope.has_begin_session() {
        tracing::debug!(session_id = %session_id, "session begin");
        Some(session_id.clone())
    } else {
        if envelope
            .header
            .as_ref()
            .and_then(|h| h.end_session.as_ref())
            .is_some()
        {
            tracing::debug!(session_id = %session_id, "session end");
        }
        incoming_session_id.clone()
    };

    let finish = |xml: &str| -> Response {
        tracing::trace!(body = %pretty_xml(xml), "response body");
        let (bytes, ct) = codec.encode_response(xml);
        tracing::debug!(content_type = ct, "response encoding");
        let mut r = Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", ct)
            .body(Body::from(bytes))
            .expect("Content-Type is always one of codec's fixed &'static str values");
        let h = r.headers_mut();
        h.insert(
            "x-ms-xmlacaps-negotiation-flags",
            HeaderValue::from_static(XmlaCodec::response_flags()),
        );
        if let Some(sid) = response_session.as_deref() {
            if let Ok(v) = HeaderValue::from_str(sid) {
                h.insert("x-ms-xmlasession-id", v);
            }
        }
        let resp_headers: String = r
            .headers()
            .iter()
            .map(|(k, v)| format!("  {}: {}", k, v.to_str().unwrap_or("<binary>")))
            .collect::<Vec<_>>()
            .join("\n");
        tracing::debug!(headers = %resp_headers, "response headers");
        r
    };

    let sid = response_session.as_deref();
    let databases = provider.list_databases();

    let xml = if let Some(execute) = &envelope.body.execute {
        tracing::debug!(
            session_id = %session_id,
            is_session_management = execute.is_session_management(),
            statement = ?execute.statement(),
            "Execute request"
        );

        let db = execute.catalog().and_then(|c| provider.database(c));

        let stmt = execute.statement().unwrap_or("");

        match parse_mdx(stmt) {
            Ok(query) => match &query.from {
                FromClause::System { table, conditions, .. } => {
                    tracing::debug!(table, "MDX $system query");
                    let result = match table.to_uppercase().as_str() {
                        "MDSCHEMA_CUBES" => {
                            let params = execute.parameters();
                            let catalog_filter = conditions
                                .iter()
                                .find(|c| c.column.eq_ignore_ascii_case("CATALOG_NAME"))
                                .map(|c| match &c.value {
                                    ConditionValue::Literal(s) => s.clone(),
                                    ConditionValue::Param(p) => {
                                        params.get(p).cloned().unwrap_or_default()
                                    }
                                });
                            let filtered: Vec<_> = match &catalog_filter {
                                Some(cat) => databases
                                    .iter()
                                    .filter(|db| db.name.eq_ignore_ascii_case(cat))
                                    .cloned()
                                    .collect(),
                                None => databases.clone(),
                            };
                            handlers::dmv_cubes_rows(&filtered)
                        }
                        "DBSCHEMA_CATALOGS" => handlers::dmv_catalogs_rows(&databases),
                        "MDSCHEMA_MEASURES" => {
                            let (cat, measures) =
                                resolve_measures(provider, &databases, db.as_deref());
                            handlers::dmv_measures_rows(&cat, &measures)
                        }
                        "MDSCHEMA_DIMENSIONS" => {
                            let (cat, _) = resolve_measures(provider, &databases, db.as_deref());
                            let tables = resolve_tables(provider, &databases, db.as_deref());
                            handlers::dmv_dimensions_rows(&cat, &tables)
                        }
                        "MDSCHEMA_HIERARCHIES" => {
                            let (cat, _) = resolve_measures(provider, &databases, db.as_deref());
                            let tables = resolve_tables(provider, &databases, db.as_deref());
                            return finish(&handlers::dmv_hierarchies(sid, &cat, &tables).0);
                        }
                        "MDSCHEMA_LEVELS" => {
                            let (cat, _) = resolve_measures(provider, &databases, db.as_deref());
                            let tables = resolve_tables(provider, &databases, db.as_deref());
                            return finish(&handlers::dmv_levels(sid, &cat, &tables).0);
                        }
                        "MDSCHEMA_KPIS" => return finish(&handlers::dmv_kpis(sid).0),
                        "TMSCHEMA_MODEL" => {
                            let name = db
                                .as_deref()
                                .map(|d| d.name().to_string())
                                .or_else(|| databases.first().map(|m| m.name.clone()))
                                .unwrap_or_default();
                            let meta = db
                                .as_deref()
                                .map(|d| d.model_meta())
                                .or_else(|| {
                                    databases
                                        .first()
                                        .and_then(|m| provider.database(&m.name))
                                        .map(|d| d.model_meta())
                                })
                                .unwrap_or_default();
                            return finish(&handlers::tmschema_model(sid, &name, &meta).0);
                        }
                        "TMSCHEMA_TABLES" => {
                            let tables = resolve_tables(provider, &databases, db.as_deref());
                            return finish(&handlers::tmschema_tables(sid, &tables).0);
                        }
                        "TMSCHEMA_COLUMNS" => {
                            let tables = resolve_tables(provider, &databases, db.as_deref());
                            return finish(&handlers::tmschema_columns(sid, &tables).0);
                        }
                        "TMSCHEMA_MEASURES" => {
                            let (_, measures) =
                                resolve_measures(provider, &databases, db.as_deref());
                            let tables = resolve_tables(provider, &databases, db.as_deref());
                            return finish(&handlers::tmschema_measures(sid, &measures, &tables).0);
                        }
                        "TMSCHEMA_RELATIONSHIPS" => {
                            let relationships =
                                resolve_relationships(provider, &databases, db.as_deref());
                            let tables = resolve_tables(provider, &databases, db.as_deref());
                            return finish(
                                &handlers::tmschema_relationships(sid, &relationships, &tables).0,
                            );
                        }
                        "TMSCHEMA_PARTITIONS" => {
                            let tables = resolve_tables(provider, &databases, db.as_deref());
                            return finish(&handlers::tmschema_partitions(sid, &tables).0);
                        }
                        "TMSCHEMA_HIERARCHIES"
                        | "TMSCHEMA_LEVELS"
                        | "TMSCHEMA_DATA_SOURCES"
                        | "TMSCHEMA_ROLES"
                        | "TMSCHEMA_ROLE_MEMBERSHIPS"
                        | "TMSCHEMA_KPIS"
                        | "TMSCHEMA_PERSPECTIVES"
                        | "TMSCHEMA_ANNOTATIONS" => {
                            return finish(&handlers::execute_empty_rowset(sid).0);
                        }
                        other => {
                            tracing::warn!(
                                other,
                                "unhandled $system table — returning empty rowset"
                            );
                            return finish(&handlers::execute_empty_rowset(sid).0);
                        }
                    };
                    handlers::render_dmv_result(sid, result).0
                }
                FromClause::Cube(cube_name) | FromClause::SubqueryCube { cube: cube_name, .. } => {
                    tracing::info!(cube = cube_name.as_str(), "MDX cube query");

                    let translation = match mdx_to_dax(&query) {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::warn!(error = %e, "MDX translation failed");
                            return finish(&handlers::execute_empty_rowset(sid).0);
                        }
                    };

                    let d = execute
                        .catalog()
                        .and_then(|c| provider.database(c))
                        .or_else(|| databases.first().and_then(|m| provider.database(&m.name)));
                    let Some(d) = d else {
                        return finish(&handlers::execute_empty_rowset(sid).0);
                    };

                    let meta = d.model_meta();
                    match translation.shape {
                        QueryShape::Scalar { .. } => {
                            let scalar_value: Option<String> = if let Some(ref dax) =
                                translation.cell_dax
                            {
                                match d.execute_dax(dax) {
                                    Ok(results) => results
                                        .into_iter()
                                        .next()
                                        .and_then(|qr| qr.rows.into_iter().next())
                                        .and_then(|row| row.into_iter().next().flatten()),
                                    Err(e) => {
                                        tracing::warn!(error = %e, "MDX scalar cell_dax failed");
                                        None
                                    }
                                }
                            } else {
                                None
                            };
                            return finish(
                                &handlers::execute_mdx_scalar(
                                    sid,
                                    &translation.cube,
                                    scalar_value.as_deref(),
                                    &translation.cell_props,
                                    &meta.last_refreshed,
                                    &meta.last_schema_update,
                                )
                                .0,
                            );
                        }

                        QueryShape::MeasuresOnly { ref measures } => {
                            let values: Vec<Option<String>> = measures
                                .iter()
                                .map(|(_, expr)| {
                                    let dax = format!("EVALUATE {{ CALCULATE({}) }}", expr);
                                    match d.execute_dax(&dax) {
                                        Ok(results) => results
                                            .into_iter()
                                            .next()
                                            .and_then(|qr| qr.rows.into_iter().next())
                                            .and_then(|row| row.into_iter().next().flatten()),
                                        Err(e) => {
                                            tracing::warn!(error = %e, "MDX meas-only-cols eval failed");
                                            None
                                        }
                                    }
                                })
                                .collect();
                            return finish(
                                &handlers::execute_mdx_meas_only_cols(
                                    sid,
                                    &translation.cube,
                                    measures,
                                    &values,
                                    &translation.cell_props,
                                    &meta.last_refreshed,
                                    &meta.last_schema_update,
                                )
                                .0,
                            );
                        }

                        QueryShape::SingleAxisCrossJoin {
                            ref dim_axis,
                            ref measures,
                            measures_first,
                        } => {
                            let n = measures.len();
                            let has_two_hier = dim_axis.second_hier.is_some();
                            let cells: Vec<(String, Option<String>, Vec<Option<String>>)> =
                                if let Some(ref dax) = translation.cell_dax {
                                    match d.execute_dax(dax) {
                                        Ok(results) => results
                                            .into_iter()
                                            .next()
                                            .map(|qr| {
                                                qr.rows
                                                    .into_iter()
                                                    .filter_map(|row| {
                                                        let h1 =
                                                            row.first().and_then(|v| v.clone())?;
                                                        if has_two_hier {
                                                            let h2 =
                                                                row.get(1).and_then(|v| v.clone());
                                                            let vals = (0..n)
                                                                .map(|i| {
                                                                    row.get(2 + i)
                                                                        .and_then(|v| v.clone())
                                                                })
                                                                .collect();
                                                            Some((h1, h2, vals))
                                                        } else {
                                                            let vals = (0..n)
                                                                .map(|i| {
                                                                    row.get(1 + i)
                                                                        .and_then(|v| v.clone())
                                                                })
                                                                .collect();
                                                            Some((h1, None, vals))
                                                        }
                                                    })
                                                    .collect()
                                            })
                                            .unwrap_or_default(),
                                        Err(e) => {
                                            tracing::warn!(error = %e, "MDX single-axis crossjoin failed");
                                            vec![]
                                        }
                                    }
                                } else {
                                    vec![]
                                };
                            return finish(
                                &handlers::execute_mdx_cellset_single_axis_crossjoin(
                                    sid,
                                    &translation.cube,
                                    dim_axis,
                                    measures,
                                    &cells,
                                    &translation.cell_props,
                                    &meta.last_refreshed,
                                    &meta.last_schema_update,
                                    measures_first,
                                )
                                .0,
                            );
                        }

                        QueryShape::SingleAxisMultiDimCrossJoin {
                            ref dims,
                            ref measures,
                            measures_position,
                        } => {
                            let n_dims = dims.len();
                            let n_meas = measures.len();
                            let cells: Vec<Vec<Option<String>>> = if let Some(ref dax) =
                                translation.cell_dax
                            {
                                match d.execute_dax(dax) {
                                    Ok(results) => results
                                        .into_iter()
                                        .next()
                                        .map(|qr| {
                                            qr.rows
                                                .into_iter()
                                                .filter_map(|row| {
                                                    row.first().and_then(|v| v.as_ref())?;
                                                    let mut combined: Vec<Option<String>> = (0
                                                        ..n_dims)
                                                        .map(|i| row.get(i).and_then(|v| v.clone()))
                                                        .collect();
                                                    combined.extend((0..n_meas).map(|i| {
                                                        row.get(n_dims + i).and_then(|v| v.clone())
                                                    }));
                                                    Some(combined)
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                    Err(e) => {
                                        tracing::warn!(error = %e, "MDX multi-dim crossjoin failed");
                                        vec![]
                                    }
                                }
                            } else {
                                vec![]
                            };
                            return finish(
                                &handlers::execute_mdx_cellset_single_axis_multi_dim_crossjoin(
                                    sid,
                                    &translation.cube,
                                    dims,
                                    measures,
                                    measures_position,
                                    &cells,
                                    &translation.cell_props,
                                    &meta.last_refreshed,
                                    &meta.last_schema_update,
                                )
                                .0,
                            );
                        }

                        QueryShape::CrossJoinMatrix {
                            ref crossjoin_dim,
                            ref plain_dim,
                            ref measures,
                            measures_first,
                            crossjoin_on_rows,
                        } => {
                            let n = measures.len();
                            let cells: Vec<(String, String, Vec<Option<String>>)> =
                                if let Some(ref dax) = translation.cell_dax {
                                    match d.execute_dax(dax) {
                                        Ok(results) => results
                                            .into_iter()
                                            .next()
                                            .map(|qr| {
                                                qr.rows
                                                    .into_iter()
                                                    .filter_map(|row| {
                                                        let c =
                                                            row.first().and_then(|v| v.clone())?;
                                                        let r = row
                                                            .get(1)
                                                            .and_then(|v| v.clone())
                                                            .unwrap_or_default();
                                                        let vals = (0..n)
                                                            .map(|i| {
                                                                row.get(2 + i)
                                                                    .and_then(|v| v.clone())
                                                            })
                                                            .collect();
                                                        Some((c, r, vals))
                                                    })
                                                    .collect()
                                            })
                                            .unwrap_or_default(),
                                        Err(e) => {
                                            tracing::warn!(error = %e, "MDX col-matrix failed");
                                            vec![]
                                        }
                                    }
                                } else {
                                    vec![]
                                };
                            return finish(
                                &handlers::execute_mdx_cellset_col_matrix(
                                    sid,
                                    &translation.cube,
                                    crossjoin_dim,
                                    plain_dim,
                                    measures,
                                    &cells,
                                    &translation.cell_props,
                                    &meta.last_refreshed,
                                    &meta.last_schema_update,
                                    measures_first,
                                    crossjoin_on_rows,
                                )
                                .0,
                            );
                        }

                        QueryShape::TwoHierWithMeasures { ref dim_axis, ref measures } => {
                            let n = measures.len();
                            let cells: Vec<(String, String, Vec<Option<String>>)> = if let Some(
                                ref dax,
                            ) =
                                translation.cell_dax
                            {
                                match d.execute_dax(dax) {
                                    Ok(results) => results
                                        .into_iter()
                                        .next()
                                        .map(|qr| {
                                            qr.rows
                                                .into_iter()
                                                .filter_map(|row| {
                                                    let h1 = row.first().and_then(|v| v.clone())?;
                                                    let h2 = row
                                                        .get(1)
                                                        .and_then(|v| v.clone())
                                                        .unwrap_or_default();
                                                    let vals = (0..n)
                                                        .map(|i| {
                                                            row.get(2 + i).and_then(|v| v.clone())
                                                        })
                                                        .collect();
                                                    Some((h1, h2, vals))
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                    Err(e) => {
                                        tracing::warn!(error = %e, "MDX two-hier-matrix failed");
                                        vec![]
                                    }
                                }
                            } else {
                                vec![]
                            };
                            return finish(
                                &handlers::execute_mdx_cellset_matrix(
                                    sid,
                                    &translation.cube,
                                    dim_axis,
                                    measures,
                                    &cells,
                                    &translation.cell_props,
                                    &meta.last_refreshed,
                                    &meta.last_schema_update,
                                )
                                .0,
                            );
                        }

                        QueryShape::DimMeasureMatrix {
                            ref dim_axis,
                            ref measures,
                            measures_on_rows,
                        } => {
                            let n = measures.len();
                            let cells: Vec<(String, Vec<Option<String>>)> = if let Some(ref dax) =
                                translation.cell_dax
                            {
                                match d.execute_dax(dax) {
                                    Ok(results) => results
                                        .into_iter()
                                        .next()
                                        .map(|qr| {
                                            qr.rows
                                                .into_iter()
                                                .filter_map(|row| {
                                                    let k = row.first().and_then(|v| v.clone())?;
                                                    let vals = (0..n)
                                                        .map(|i| {
                                                            row.get(1 + i).and_then(|v| v.clone())
                                                        })
                                                        .collect();
                                                    Some((k, vals))
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                    Err(e) => {
                                        tracing::warn!(error = %e, "MDX dim-measure-matrix failed");
                                        vec![]
                                    }
                                }
                            } else {
                                vec![]
                            };
                            if measures_on_rows {
                                return finish(
                                    &handlers::execute_mdx_cellset_meas_on_rows(
                                        sid,
                                        &translation.cube,
                                        dim_axis,
                                        measures,
                                        &cells,
                                        &translation.cell_props,
                                        &meta.last_refreshed,
                                        &meta.last_schema_update,
                                    )
                                    .0,
                                );
                            } else {
                                return finish(
                                    &handlers::execute_mdx_cellset_meas_on_cols(
                                        sid,
                                        &translation.cube,
                                        dim_axis,
                                        measures,
                                        &cells,
                                        &translation.cell_props,
                                        &meta.last_refreshed,
                                        &meta.last_schema_update,
                                    )
                                    .0,
                                );
                            }
                        }

                        QueryShape::TwoDimAxes { ref col_axis, ref row_axis, ref measure_name } => {
                            if measure_name.is_some() {
                                let cells: Vec<(String, String, Option<String>)> = if let Some(
                                    ref dax,
                                ) =
                                    translation.cell_dax
                                {
                                    match d.execute_dax(dax) {
                                        Ok(results) => results
                                            .into_iter()
                                            .next()
                                            .map(|qr| {
                                                qr.rows
                                                    .into_iter()
                                                    .filter_map(|row| {
                                                        let c =
                                                            row.first().and_then(|v| v.clone())?;
                                                        let r = row
                                                            .get(1)
                                                            .and_then(|v| v.clone())
                                                            .unwrap_or_default();
                                                        let val =
                                                            row.get(2).and_then(|v| v.clone());
                                                        Some((c, r, val))
                                                    })
                                                    .collect()
                                            })
                                            .unwrap_or_default(),
                                        Err(e) => {
                                            tracing::warn!(error = %e, "MDX two-dim-axis-measure failed");
                                            vec![]
                                        }
                                    }
                                } else {
                                    vec![]
                                };
                                return finish(
                                    &handlers::execute_mdx_cellset_two_dim_axis_measure(
                                        sid,
                                        &translation.cube,
                                        col_axis,
                                        row_axis,
                                        &cells,
                                        &translation.cell_props,
                                        &meta.last_refreshed,
                                        &meta.last_schema_update,
                                    )
                                    .0,
                                );
                            } else {
                                let pairs: Vec<(String, String)> = if let Some(ref dax) =
                                    translation.cell_dax
                                {
                                    match d.execute_dax(dax) {
                                        Ok(results) => results
                                            .into_iter()
                                            .next()
                                            .map(|qr| {
                                                qr.rows
                                                    .into_iter()
                                                    .filter_map(|row| {
                                                        let c =
                                                            row.first().and_then(|v| v.clone())?;
                                                        let r = row
                                                            .get(1)
                                                            .and_then(|v| v.clone())
                                                            .unwrap_or_default();
                                                        Some((c, r))
                                                    })
                                                    .collect()
                                            })
                                            .unwrap_or_default(),
                                        Err(e) => {
                                            tracing::warn!(error = %e, "MDX two-dim-axis failed");
                                            vec![]
                                        }
                                    }
                                } else {
                                    vec![]
                                };
                                return finish(
                                    &handlers::execute_mdx_cellset_two_dim_axis(
                                        sid,
                                        &translation.cube,
                                        col_axis,
                                        row_axis,
                                        &pairs,
                                        &translation.cell_props,
                                        &meta.last_refreshed,
                                        &meta.last_schema_update,
                                    )
                                    .0,
                                );
                            }
                        }

                        QueryShape::TwoHierDim { ref axis, ref measure_name } => {
                            let has_measure = measure_name.is_some();
                            let cells: Vec<(String, String, Option<String>)> =
                                if let Some(ref dax) = translation.cell_dax {
                                    match d.execute_dax(dax) {
                                        Ok(results) => results
                                            .into_iter()
                                            .next()
                                            .map(|qr| {
                                                qr.rows
                                                    .into_iter()
                                                    .filter_map(|row| {
                                                        let h1 =
                                                            row.first().and_then(|v| v.clone())?;
                                                        let h2 = row
                                                            .get(1)
                                                            .and_then(|v| v.clone())
                                                            .unwrap_or_default();
                                                        let val = if has_measure {
                                                            row.get(2).and_then(|v| v.clone())
                                                        } else {
                                                            None
                                                        };
                                                        Some((h1, h2, val))
                                                    })
                                                    .collect()
                                            })
                                            .unwrap_or_default(),
                                        Err(e) => {
                                            tracing::warn!(error = %e, "MDX two-hier failed");
                                            vec![]
                                        }
                                    }
                                } else {
                                    vec![]
                                };
                            return finish(
                                &handlers::execute_mdx_cellset_two_hier(
                                    sid,
                                    &translation.cube,
                                    axis,
                                    &cells,
                                    &translation.cell_props,
                                    &meta.last_refreshed,
                                    &meta.last_schema_update,
                                )
                                .0,
                            );
                        }

                        QueryShape::SingleDim { ref axis, ref measure_name, has_measure_axis } => {
                            let total_value: Option<String> =
                                if let Some(ref dax) = translation.total_dax {
                                    match d.execute_dax(dax) {
                                        Ok(results) => results
                                            .into_iter()
                                            .next()
                                            .and_then(|qr| qr.rows.into_iter().next())
                                            .and_then(|row| row.into_iter().next().flatten()),
                                        Err(e) => {
                                            tracing::warn!(error = %e, "MDX total_dax failed");
                                            None
                                        }
                                    }
                                } else {
                                    None
                                };
                            let leaf_members: Vec<(String, String)> =
                                if let Some(ref dax) = translation.cell_dax {
                                    match d.execute_dax(dax) {
                                        Ok(results) => results
                                            .into_iter()
                                            .next()
                                            .map(|qr| {
                                                qr.rows
                                                    .into_iter()
                                                    .filter_map(|row| {
                                                        let key =
                                                            row.first().and_then(|v| v.clone())?;
                                                        let val = row
                                                            .get(1)
                                                            .and_then(|v| v.clone())
                                                            .unwrap_or_default();
                                                        if translation.non_empty
                                                            && measure_name.is_some()
                                                            && val.is_empty()
                                                        {
                                                            return None;
                                                        }
                                                        Some((key, val))
                                                    })
                                                    .collect()
                                            })
                                            .unwrap_or_default(),
                                        Err(e) => {
                                            tracing::warn!(error = %e, "MDX cell_dax failed");
                                            vec![]
                                        }
                                    }
                                } else {
                                    vec![]
                                };
                            return finish(
                                &handlers::execute_mdx_cellset(
                                    sid,
                                    &translation.cube,
                                    axis,
                                    measure_name.as_deref(),
                                    total_value.as_deref(),
                                    &leaf_members,
                                    &translation.cell_props,
                                    &meta.last_refreshed,
                                    &meta.last_schema_update,
                                    has_measure_axis,
                                )
                                .0,
                            );
                        }
                    }

                    #[allow(unreachable_code)]
                    handlers::execute_empty_rowset(sid).0
                }
            },
            Err(_) => {
                if let Some(stmt) = execute.statement() {
                    let upper = stmt.trim().to_uppercase();
                    if upper.starts_with("EVALUATE") || upper.starts_with("DEFINE") {
                        let db = execute
                            .catalog()
                            .and_then(|c| provider.database(c))
                            .or_else(|| databases.first().and_then(|m| provider.database(&m.name)));

                        let Some(d) = db else {
                            return finish(&handlers::execute_empty_rowset(sid).0);
                        };

                        let cat = d.name().to_string();
                        let wants_metrics = execute.wants_execution_metrics();
                        let parse_ms = t_request.elapsed().as_millis() as u64;
                        let t0 = std::time::Instant::now();
                        let xml = match d.execute_dax(stmt) {
                            Ok(qr) => {
                                let dax_ms = t0.elapsed().as_millis() as u64;
                                let t_serialize = std::time::Instant::now();
                                let result = handlers::execute_query_result(
                                    sid,
                                    &cat,
                                    qr,
                                    wants_metrics.then_some(dax_ms),
                                )
                                .0;
                                let serialize_ms = t_serialize.elapsed().as_millis() as u64;
                                tracing::info!(
                                    catalog = %cat,
                                    parse_ms,
                                    dax_ms,
                                    serialize_ms,
                                    "DAX executed"
                                );
                                result
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    statement = %stmt,
                                    "DAX execution failed"
                                );
                                handlers::execute_fault(sid, &e).0
                            }
                        };
                        return finish(&xml);
                    }
                }
                handlers::execute_ok(sid).0
            }
        }
    } else if let Some(discover) = &envelope.body.discover {
        let request_type = discover.request_type.as_str();
        tracing::debug!(request_type, session_id = %session_id, "Discover request");

        let db = discover
            .resolved_catalog()
            .and_then(|c| provider.database(c));

        match request_type {
            "DISCOVER_DATASOURCES" => handlers::discover_datasources(sid, config).0,
            "DISCOVER_PROPERTIES" => {
                let catalog = discover
                    .catalog()
                    .or_else(|| databases.first().map(|d| d.name.as_str()));
                handlers::discover_properties(
                    sid,
                    discover.property_name_restriction().as_deref(),
                    catalog,
                    config,
                )
                .0
            }
            "DISCOVER_SCHEMA_ROWSETS" => {
                handlers::discover_schema_rowsets(sid, discover.schema_name_restriction()).0
            }
            "DISCOVER_KEYWORDS" => handlers::empty_ok(sid).0,
            "DISCOVER_LITERALS" => handlers::discover_literals(sid).0,
            "MDSCHEMA_SETS" => handlers::discover_mdschema_sets(sid).0,
            "DISCOVER_CATALOGS" => handlers::discover_catalogs(sid, &databases).0,
            "DBSCHEMA_CATALOGS" => handlers::discover_catalogs(sid, &databases).0,
            "MDSCHEMA_CUBES" => {
                let filtered: Vec<_> = match discover.resolved_catalog() {
                    Some(cat) => databases
                        .iter()
                        .filter(|db| db.name.eq_ignore_ascii_case(cat))
                        .cloned()
                        .collect(),
                    None => databases.clone(),
                };
                handlers::discover_cubes(sid, discover.cube_source_restriction(), &filtered).0
            }
            "MDSCHEMA_DIMENSIONS" => {
                let (cat, measures) = resolve_measures(provider, &databases, db.as_deref());
                let tables = resolve_tables(provider, &databases, db.as_deref());
                handlers::discover_dimensions(sid, &cat, &tables, &measures).0
            }
            "MDSCHEMA_HIERARCHIES" => {
                let (cat, _) = resolve_measures(provider, &databases, db.as_deref());
                let tables = resolve_tables(provider, &databases, db.as_deref());
                handlers::discover_hierarchies(sid, &cat, &tables).0
            }
            "MDSCHEMA_LEVELS" => {
                let (cat, _) = resolve_measures(provider, &databases, db.as_deref());
                let tables = resolve_tables(provider, &databases, db.as_deref());
                handlers::discover_levels(sid, &cat, &tables).0
            }
            "MDSCHEMA_FUNCTIONS" => {
                handlers::discover_functions(sid, discover.origin_restriction()).0
            }
            "DISCOVER_INSTANCES" => handlers::empty_ok(sid).0,
            "MDSCHEMA_MEASURES" => {
                let (cat, measures) = resolve_measures(provider, &databases, db.as_deref());
                handlers::discover_measures(sid, &cat, &measures).0
            }
            "MDSCHEMA_KPIS" => handlers::discover_mdschema_kpis(sid).0,
            "MDSCHEMA_MEASUREGROUPS" => {
                let (cat, _) = resolve_measures(provider, &databases, db.as_deref());
                let tables = resolve_tables(provider, &databases, db.as_deref());
                handlers::discover_mdschema_measuregroups(sid, &cat, &tables).0
            }
            "MDSCHEMA_MEASUREGROUP_DIMENSIONS" => {
                let (cat, _) = resolve_measures(provider, &databases, db.as_deref());
                let tables = resolve_tables(provider, &databases, db.as_deref());
                let relationships = resolve_relationships(provider, &databases, db.as_deref());
                handlers::discover_mdschema_measuregroup_dimensions(
                    sid,
                    &cat,
                    &tables,
                    &relationships,
                )
                .0
            }
            "MDSCHEMA_PROPERTIES" => {
                let (cat, _) = resolve_measures(provider, &databases, db.as_deref());
                let tables = resolve_tables(provider, &databases, db.as_deref());
                handlers::discover_mdschema_properties(
                    sid,
                    &cat,
                    &tables,
                    discover.property_type_restriction(),
                )
                .0
            }
            "DBSCHEMA_TABLES" => {
                let (cat, _) = resolve_measures(provider, &databases, db.as_deref());
                let tables = resolve_tables(provider, &databases, db.as_deref());
                let meta = db
                    .as_deref()
                    .map(|d| d.model_meta())
                    .or_else(|| {
                        databases
                            .first()
                            .and_then(|m| provider.database(&m.name))
                            .map(|d| d.model_meta())
                    })
                    .unwrap_or_default();
                handlers::discover_dbschema_tables(
                    sid,
                    &cat,
                    &tables,
                    &meta.created_timestamp,
                    &meta.last_refreshed,
                )
                .0
            }
            "DBSCHEMA_COLUMNS" => handlers::empty_ok(sid).0,
            "DISCOVER_XML_METADATA" => {
                let expansion = discover.object_expansion_restriction();
                let (tom, meta) = if let Some(d) = db.as_deref() {
                    let m = d.model_meta();
                    let t = handlers::build_tom_xml(
                        d.name(),
                        &d.list_tables(),
                        &d.list_measures(),
                        &d.list_relationships(),
                        &m,
                    );
                    (Some(t), Some(m))
                } else if let Some(d) = databases.first().and_then(|m| provider.database(&m.name)) {
                    let m = d.model_meta();
                    let t = handlers::build_tom_xml(
                        d.name(),
                        &d.list_tables(),
                        &d.list_measures(),
                        &d.list_relationships(),
                        &m,
                    );
                    (Some(t), Some(m))
                } else {
                    (None, None)
                };
                handlers::discover_xml_metadata(
                    sid,
                    expansion,
                    &databases,
                    tom,
                    meta.as_ref(),
                    config,
                )
                .0
            }
            "TMSCHEMA_MODEL" => {
                let name = db
                    .as_deref()
                    .map(|d| d.name().to_string())
                    .or_else(|| databases.first().map(|m| m.name.clone()))
                    .unwrap_or_default();
                let meta = db
                    .as_deref()
                    .map(|d| d.model_meta())
                    .or_else(|| {
                        databases
                            .first()
                            .and_then(|m| provider.database(&m.name))
                            .map(|d| d.model_meta())
                    })
                    .unwrap_or_default();
                handlers::tmschema_model(sid, &name, &meta).0
            }
            "TMSCHEMA_TABLES" => {
                let tables = resolve_tables(provider, &databases, db.as_deref());
                handlers::tmschema_tables(sid, &tables).0
            }
            "TMSCHEMA_COLUMNS" => {
                let tables = resolve_tables(provider, &databases, db.as_deref());
                handlers::tmschema_columns(sid, &tables).0
            }
            "TMSCHEMA_MEASURES" => {
                let (_, measures) = resolve_measures(provider, &databases, db.as_deref());
                let tables = resolve_tables(provider, &databases, db.as_deref());
                handlers::tmschema_measures(sid, &measures, &tables).0
            }
            "TMSCHEMA_RELATIONSHIPS" => {
                let relationships = resolve_relationships(provider, &databases, db.as_deref());
                let tables = resolve_tables(provider, &databases, db.as_deref());
                handlers::tmschema_relationships(sid, &relationships, &tables).0
            }
            "TMSCHEMA_PARTITIONS" => {
                let tables = resolve_tables(provider, &databases, db.as_deref());
                handlers::tmschema_partitions(sid, &tables).0
            }
            "DISCOVER_CSDL_METADATA" => {
                let (cat, measures) = resolve_measures(provider, &databases, db.as_deref());
                let tables = resolve_tables(provider, &databases, db.as_deref());
                let relationships = resolve_relationships(provider, &databases, db.as_deref());
                let version = discover.csdl_version_restriction().unwrap_or("2.0");
                handlers::discover_csdl_metadata(
                    sid,
                    &cat,
                    &tables,
                    &measures,
                    &relationships,
                    version,
                )
                .0
            }
            "MDSCHEMA_MEMBERS" => {
                let (cat, _) = resolve_measures(provider, &databases, db.as_deref());
                let tables = resolve_tables(provider, &databases, db.as_deref());
                let member_uname = discover.member_unique_name_restriction().unwrap_or("");
                let tree_op = discover.tree_op_restriction().unwrap_or(0);
                handlers::discover_members(sid, &cat, &tables, member_uname, tree_op).0
            }
            "TMSCHEMA_HIERARCHIES"
            | "TMSCHEMA_LEVELS"
            | "TMSCHEMA_DATA_SOURCES"
            | "TMSCHEMA_ROLES"
            | "TMSCHEMA_ROLE_MEMBERSHIPS"
            | "TMSCHEMA_KPIS"
            | "TMSCHEMA_PERSPECTIVES"
            | "TMSCHEMA_ANNOTATIONS" => handlers::empty_ok(sid).0,
            _ => {
                tracing::warn!(
                    request_type,
                    "unrecognised RequestType — returning empty rowset"
                );
                handlers::empty_ok(sid).0
            }
        }
    } else {
        tracing::warn!("SOAP body contains neither Discover nor Execute");
        handlers::empty_ok(sid).0
    };

    finish(&xml)
}

fn resolve_tables(
    provider: &Arc<dyn ServerProvider>,
    databases: &[crate::server::provider::DatabaseMeta],
    db: Option<&dyn crate::server::provider::DatabaseProvider>,
) -> Vec<crate::server::provider::TableMeta> {
    if let Some(d) = db {
        return d.list_tables();
    }
    if let Some(first) = databases.first().and_then(|m| provider.database(&m.name)) {
        return first.list_tables();
    }
    vec![]
}

fn resolve_relationships(
    provider: &Arc<dyn ServerProvider>,
    databases: &[crate::server::provider::DatabaseMeta],
    db: Option<&dyn crate::server::provider::DatabaseProvider>,
) -> Vec<crate::server::provider::RelationshipMeta> {
    if let Some(d) = db {
        return d.list_relationships();
    }
    if let Some(first) = databases.first().and_then(|m| provider.database(&m.name)) {
        return first.list_relationships();
    }
    vec![]
}

fn resolve_measures(
    provider: &Arc<dyn ServerProvider>,
    databases: &[crate::server::provider::DatabaseMeta],
    db: Option<&dyn crate::server::provider::DatabaseProvider>,
) -> (String, Vec<crate::server::provider::MeasureMeta>) {
    if let Some(d) = db {
        return (d.name().to_string(), d.list_measures());
    }
    if let Some(first) = databases.first().and_then(|m| provider.database(&m.name)) {
        return (first.name().to_string(), first.list_measures());
    }
    (String::new(), vec![])
}

fn pretty_xml(xml: &str) -> String {
    use quick_xml::{events::Event, Reader, Writer};
    use std::io::Cursor;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(e) => {
                let _ = writer.write_event(e);
            }
            Err(_) => return xml.to_string(),
        }
    }
    String::from_utf8(writer.into_inner().into_inner()).unwrap_or_else(|_| xml.to_string())
}
