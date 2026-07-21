use std::sync::Arc;

use crate::app::rag::ingestion_service::IngestionService;
use crate::app::rag::retrieval_service::RetrievalService;

use super::{
    BootstrapContext, rag_ingestion_provider::build_rag_ingestion_service,
    rag_retrieval_provider::build_rag_retrieval_service,
};

pub struct RagServices {
    pub retrieval: Arc<RetrievalService>,
    pub ingestion: Arc<IngestionService>,
}

pub fn build_rag_services(ctx: &BootstrapContext<'_>) -> RagServices {
    RagServices {
        retrieval: build_rag_retrieval_service(ctx),
        ingestion: build_rag_ingestion_service(ctx),
    }
}
