use std::{collections::HashMap, sync::Arc};

use actix_multipart::Multipart;
use actix_web::{delete, post, web, HttpResponse};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    datasets::{
        datapoints::{self, Datapoint},
        utils::{index_new_points, read_multipart_file, ParsedFile},
    },
    db::{self, datasets, DB},
    routes::ResponseResult,
    semantic_search::{SemanticSearch, SemanticSearchTrait},
};

const BATCH_SIZE: usize = 50;

#[delete("datasets/{dataset_id}")]
async fn delete_dataset(
    db: web::Data<DB>,
    path: web::Path<(Uuid, Uuid)>,
    semantic_search: web::Data<Arc<SemanticSearch>>,
) -> ResponseResult {
    let (project_id, dataset_id) = path.into_inner();

    datasets::delete_dataset(&db.pool, dataset_id).await?;

    semantic_search
        .delete_embeddings(
            &project_id.to_string(),
            vec![HashMap::from([(
                "datasource_id".to_string(),
                dataset_id.to_string(),
            )])],
        )
        .await?;

    Ok(HttpResponse::Ok().finish())
}

// NOTE: this endpoint currently assumes one file upload.
// If we want to support multiple files, we will need to keep a list of filename -> bytes links.
// and potentially batch process, so that we don't hold enormous files in memory
#[post("datasets/{dataset_id}/file-upload")]
async fn upload_datapoint_file(
    payload: Multipart,
    path: web::Path<(Uuid, Uuid)>,
    db: web::Data<DB>,
    semantic_search: web::Data<Arc<SemanticSearch>>,
) -> ResponseResult {
    let (project_id, dataset_id) = path.into_inner();
    let db = db.into_inner();

    let ParsedFile { filename, bytes } = read_multipart_file(payload).await?;

    let Some(dataset) = db::datasets::get_dataset(&db.pool, project_id, dataset_id).await? else {
        return Ok(HttpResponse::NotFound().body("Dataset not found"));
    };

    let indexed_on = dataset.indexed_on.clone();

    let datapoints =
        datapoints::insert_datapoints_from_file(&bytes, &filename, dataset_id, db.clone()).await?;

    if indexed_on.is_some() {
        index_new_points(
            datapoints.clone(),
            semantic_search.as_ref().clone(),
            project_id.to_string(),
            indexed_on,
        )
        .await?;
    }

    Ok(HttpResponse::Ok().json(datapoints))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateDatapointsRequest {
    datapoints: Vec<serde_json::Value>,
    indexed_on: String,
}

#[post("datasets/{dataset_id}/datapoints")]
async fn create_datapoint_embeddings(
    path: web::Path<(Uuid, Uuid)>,
    req: web::Json<CreateDatapointsRequest>,
    semantic_search: web::Data<Arc<SemanticSearch>>,
) -> ResponseResult {
    let (project_id, dataset_id) = path.into_inner();
    let req = req.into_inner();
    let indexed_on = req.indexed_on;
    let input_datapoints = req.datapoints;

    let datapoints = input_datapoints
        .iter()
        .filter_map(|value| Datapoint::try_from_raw_value(dataset_id.to_owned(), value))
        .collect::<Vec<_>>();

    index_new_points(
        datapoints.clone(),
        semantic_search.as_ref().clone(),
        project_id.to_string(),
        Some(indexed_on),
    )
    .await?;

    Ok(HttpResponse::Ok().json(datapoints))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateDatapointRequest {
    data: Value,
    target: Option<Value>,
    metadata: HashMap<String, Value>,
    indexed_on: String,
}

// TODO: potentially split this into two endpoints:
// 1. update_datapoint_embeddings
// 2. update_datapoint_metadata
// In the second one, we don't need to delete the embeddings, we can just update the qdrant payload
// https://api.qdrant.tech/api-reference/points/overwrite-payload
#[post("datasets/{dataset_id}/datapoints/{datapoint_id}")]
async fn update_datapoint_embeddings(
    path: web::Path<(Uuid, Uuid, Uuid)>,
    req: web::Json<UpdateDatapointRequest>,
    semantic_search: web::Data<Arc<SemanticSearch>>,
) -> ResponseResult {
    let (project_id, dataset_id, datapoint_id) = path.into_inner();
    let req = req.into_inner();

    semantic_search
        .delete_embeddings(
            &project_id.to_string(),
            vec![HashMap::from([(
                "id".to_string(),
                datapoint_id.to_string(),
            )])],
        )
        .await?;

    let updated_datapoint = Datapoint {
        id: datapoint_id,
        dataset_id,
        data: req.data,
        target: req.target,
        metadata: req.metadata,
    };

    index_new_points(
        vec![updated_datapoint.clone()],
        semantic_search.as_ref().clone(),
        project_id.to_string(),
        Some(req.indexed_on),
    )
    .await?;

    Ok(HttpResponse::Ok().json(updated_datapoint))
}

#[derive(Deserialize)]
pub struct DeleteDatapointRequest {
    pub ids: Vec<Uuid>,
}

#[delete("datasets/{dataset_id}/datapoints")]
async fn delete_datapoint_embeddings(
    path: web::Path<(Uuid, Uuid)>,
    req: web::Json<DeleteDatapointRequest>,
    semantic_search: web::Data<Arc<SemanticSearch>>,
) -> ResponseResult {
    let (project_id, dataset_id) = path.into_inner();
    let datapoint_ids = req.into_inner().ids;

    semantic_search
        .delete_embeddings(
            &project_id.to_string(),
            datapoint_ids
                .iter()
                .map(|id| {
                    HashMap::from([
                        ("id".to_string(), id.to_string()),
                        ("datasource_id".to_string(), dataset_id.to_string()),
                    ])
                })
                .collect::<Vec<_>>(),
        )
        .await?;

    Ok(HttpResponse::Ok().finish())
}

#[delete("datasets/{dataset_id}/datapoints/all")]
async fn delete_all_datapoints(
    path: web::Path<(Uuid, Uuid)>,
    db: web::Data<DB>,
    semantic_search: web::Data<Arc<SemanticSearch>>,
) -> ResponseResult {
    let (project_id, dataset_id) = path.into_inner();

    let deleted_dp_ids = db::datapoints::delete_all_datapoints(&db.pool, &dataset_id).await?;

    semantic_search
        .delete_embeddings(
            &project_id.to_string(),
            deleted_dp_ids
                .iter()
                .map(|id| HashMap::from([("id".to_string(), id.to_string())]))
                .collect::<Vec<_>>(),
        )
        .await?;

    Ok(HttpResponse::Ok().finish())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexDatasetRequest {
    index_column: Option<String>,
}

#[post("datasets/{dataset_id}/index")]
async fn index_dataset(
    db: web::Data<DB>,
    path: web::Path<(Uuid, Uuid)>,
    request: web::Json<IndexDatasetRequest>,
    semantic_search: web::Data<Arc<SemanticSearch>>,
) -> ResponseResult {
    let (project_id, dataset_id) = path.into_inner();
    let index_column = &request.index_column;
    let Some(dataset) = db::datasets::get_dataset(&db.pool, project_id, dataset_id).await? else {
        return Ok(HttpResponse::NotFound().body("Dataset not found"));
    };

    if &dataset.indexed_on == index_column {
        return Ok(HttpResponse::Ok().json(dataset));
    }

    let datapoints = db::datapoints::get_full_datapoints(&db.pool, dataset_id, None, None).await?;

    // First, delete old embeddings
    if dataset.indexed_on.is_some() {
        semantic_search
            .delete_embeddings(
                &project_id.to_string(),
                vec![HashMap::from([(
                    "datasource_id".to_string(),
                    dataset_id.to_string(),
                )])],
            )
            .await?;
    }
    for batch in datapoints.chunks(BATCH_SIZE) {
        // Then, index all embeddings
        if index_column.is_some() {
            index_new_points(
                batch
                    .iter()
                    .map(|dp| dp.to_owned().into())
                    .collect::<Vec<_>>(),
                semantic_search.as_ref().clone(),
                project_id.to_string(),
                index_column.clone(),
            )
            .await?;
        }
    }

    let dataset =
        db::datasets::update_index_column(&db.pool, dataset_id, index_column.clone()).await?;

    Ok(HttpResponse::Ok().json(dataset))
}
