/// Main LSP server implementation for Taida Lang.
///
/// Implements the Language Server Protocol using tower-lsp.
/// Features:
/// - Diagnostics on open/save/change (parse errors + type errors)
/// - Hover for type information + doc comments
/// - Context-aware completion (variables, functions, types, molds, prelude)
use std::collections::HashMap;
use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use super::completion;
use super::diagnostics;
use super::hover;
use crate::version::taida_version;

/// State for each open document.
struct DocumentState {
    content: String,
}

/// The Taida LSP backend.
pub struct TaidaBackend {
    client: Client,
    documents: Mutex<HashMap<Url, DocumentState>>,
}

impl TaidaBackend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: Mutex::new(HashMap::new()),
        }
    }

    /// Analyze a document and publish diagnostics.
    async fn publish_diagnostics(&self, uri: Url, source: &str) {
        let result = diagnostics::analyze(source);
        self.client
            .publish_diagnostics(uri, result.diagnostics, None)
            .await;
    }

    /// Get document content by URI.
    fn get_document_content(&self, uri: &Url) -> Option<String> {
        let docs = self.documents.lock().unwrap_or_else(|e| e.into_inner());
        docs.get(uri).map(|d| d.content.clone())
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for TaidaBackend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        ".".to_string(),
                        ">".to_string(),
                        "<".to_string(),
                        "|".to_string(),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "taida-lsp".to_string(),
                version: Some(taida_version().to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Taida LSP server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let content = params.text_document.text.clone();

        // Store document
        {
            let mut docs = self.documents.lock().unwrap_or_else(|e| e.into_inner());
            docs.insert(
                uri.clone(),
                DocumentState {
                    content: content.clone(),
                },
            );
        }

        // Publish diagnostics
        self.publish_diagnostics(uri, &content).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();

        // With FULL sync, we get the entire content
        if let Some(change) = params.content_changes.into_iter().last() {
            let content = change.text.clone();

            // Update stored document
            {
                let mut docs = self.documents.lock().unwrap_or_else(|e| e.into_inner());
                docs.insert(
                    uri.clone(),
                    DocumentState {
                        content: content.clone(),
                    },
                );
            }

            // Publish diagnostics
            self.publish_diagnostics(uri, &content).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;

        // Re-analyze on save
        if let Some(content) = self.get_document_content(&uri) {
            self.publish_diagnostics(uri, &content).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let mut docs = self.documents.lock().unwrap_or_else(|e| e.into_inner());
        docs.remove(&params.text_document.uri);
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        if let Some(content) = self.get_document_content(uri)
            && let Some(info) = hover::get_hover_info(&content, position)
        {
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: info,
                }),
                range: None,
            }));
        }

        Ok(None)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let source = self.get_document_content(uri);
        let items = completion::get_completions(&params, source.as_deref());
        Ok(Some(CompletionResponse::Array(items)))
    }
}

/// Run the LSP server on stdin/stdout.
pub async fn run_server() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = tower_lsp::LspService::new(TaidaBackend::new);
    tower_lsp::Server::new(stdin, stdout, socket)
        .serve(service)
        .await;
}
