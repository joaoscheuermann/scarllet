use scarllet_proto::proto::core_event;
use scarllet_proto::proto::{CoreEvent, ProviderInfoEvent};
use scarllet_sdk::config::ScarlletConfig;

/// Builds a `CoreEvent::ProviderInfo` from the current configuration.
///
/// Returns an event with empty fields when no active provider is configured.
pub(crate) fn build_provider_info_event(cfg: &ScarlletConfig) -> CoreEvent {
    let provider = match cfg.active_provider() {
        Some(p) => p,
        None => {
            return CoreEvent {
                payload: Some(core_event::Payload::ProviderInfo(ProviderInfoEvent {
                    provider_name: String::new(),
                    model: String::new(),
                    reasoning_effort: String::new(),
                })),
            };
        }
    };

    CoreEvent {
        payload: Some(core_event::Payload::ProviderInfo(ProviderInfoEvent {
            provider_name: provider.name.clone(),
            model: provider.model.clone(),
            reasoning_effort: provider
                .reasoning_effort()
                .unwrap_or_default()
                .to_string(),
        })),
    }
}
