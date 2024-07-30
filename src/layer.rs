use std::marker::PhantomData;
use std::time::SystemTime;
use std::{pin::Pin, sync::Arc};

#[allow(unused_imports)] // Many imports are used exclusively by feature-gated code
use tracing::metadata::LevelFilter;
use tracing::{span, Subscriber};
#[allow(unused_imports)]
use tracing_subscriber::filter::{combinator::And, FilterExt, Filtered, Targets};
#[allow(unused_imports)]
use tracing_subscriber::layer::Filter;
use tracing_subscriber::{registry::LookupSpan, Layer};

use crate::_details::{EtwFilter, EtwLayer};
use crate::native::{EventWriter, GuidWrapper, ProviderTypes};
use crate::{map_level, native};
use crate::values::*;
use crate::statics::*;

pub struct LayerBuilder<Mode>
where
    Mode: ProviderTypes
{
    pub(crate) provider_name: String,
    pub(crate) provider_id: GuidWrapper,
    pub(crate) provider_group: Option<Mode::ProviderGroupType>,
    pub(crate) default_keyword: u64,
    _m: PhantomData<Mode>,
}

impl LayerBuilder<native::Provider> {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(name: &str) -> LayerBuilder<native::Provider> {
        LayerBuilder::<native::Provider> {
            provider_name: name.to_owned(),
            provider_id: GuidWrapper::from_name(name),
            provider_group: None,
            default_keyword: 1,
            _m: PhantomData,
        }
    }
}

impl LayerBuilder<native::common_schema::Provider> {
    /// For advanced scenarios.
    /// Emit events that follow the Common Schema 4.0 mapping.
    /// Recommended only for compatibility with specialized event consumers.
    /// Most ETW consumers will not benefit from events in this schema, and
    /// may perform worse. Common Schema events are much slower to generate
    /// and should not be enabled unless absolutely necessary.
    #[cfg(feature = "common_schema")]
    pub fn new_common_schema_events(
        name: &str,
    ) -> LayerBuilder<native::common_schema::Provider> {
        LayerBuilder::<native::common_schema::Provider> {
            provider_name: name.to_owned(),
            provider_id: GuidWrapper::from_name(name),
            provider_group: None,
            default_keyword: 1,
            _m: PhantomData,
        }
    }
}

impl<Mode> LayerBuilder<Mode>
where
    Mode: ProviderTypes + 'static,
{
    /// For advanced scenarios.
    /// Assign a provider ID to the ETW provider rather than use
    /// one generated from the provider name.
    pub fn with_provider_id<G>(mut self, guid: &G) -> Self
    where
        for<'a> &'a G: Into<GuidWrapper>
    {
        self.provider_id = guid.into();
        self
    }

    /// Get the current provider ID that will be used for the ETW provider.
    /// This is a convenience function to help with tools that do not implement
    /// the standard provider name to ID algorithm.
    pub fn get_provider_id(&self) -> &GuidWrapper {
        &self.provider_id
    }

    pub fn with_default_keyword(mut self, kw: u64) -> Self {
        self.default_keyword = kw;
        self
    }

    /// For advanced scenarios.
    /// Set the provider group to join this provider to.
    pub fn with_provider_group<G>(mut self, group_id: &G) -> Self
    where
        for <'a> &'a G: Into<Mode::ProviderGroupType>,
    {
        self.provider_group = Some(group_id.into());
        self
    }

    fn validate_config(&self) {
        match &self.provider_group {
            None => (),
            Some(value) => Mode::assert_valid(value)
        }

        #[cfg(target_os = "linux")]
        if self
            .provider_name
            .contains(|f: char| !f.is_ascii_alphanumeric())
        {
            // The perf command is very particular about the provider names it accepts.
            // The Linux kernel itself cares less, and other event consumers should also presumably not need this check.
            //panic!("Linux provider names must be ASCII alphanumeric");
        }
    }

    #[cfg(not(feature = "global_filter"))]
    fn build_target_filter(&self, target: &'static str) -> Targets {
        let mut targets = Targets::new().with_target(&self.provider_name, LevelFilter::TRACE);

        #[cfg(target_os = "linux")]
        match self.provider_group {
            None => {}
            Some(ref name) => {
                targets = targets.with_target(Mode::get_provider_group(name), LevelFilter::TRACE);
            }
        }

        if !target.is_empty() {
            targets = targets.with_target(target, LevelFilter::TRACE)
        }

        targets
    }

    fn build_layer<S>(&self) -> EtwLayer<S, Mode>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
        Mode::Provider: EventWriter<Mode> + 'static,
    {
        EtwLayer::<S, Mode> {
            provider: Mode::Provider::new(
                &self.provider_name,
                &self.provider_id,
                &self.provider_group,
                self.default_keyword,
            ),
            default_keyword: self.default_keyword,
            _p: PhantomData,
        }
    }

    #[cfg(not(feature = "global_filter"))]
    fn build_filter<S>(&self, provider: Pin<Arc<Mode::Provider>>) -> EtwFilter<S, Mode>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
        Mode::Provider: EventWriter<Mode> + 'static,
    {
        EtwFilter::<S, Mode> {
            provider,
            default_keyword: self.default_keyword,
            _p: PhantomData,
            _m: PhantomData
        }
    }

    #[allow(clippy::type_complexity)]
    #[cfg(not(feature = "global_filter"))]
    pub fn build_with_target<S>(
        self,
        target: &'static str,
    ) -> Filtered<EtwLayer<S, Mode>, And<EtwFilter<S, Mode>, Targets, S>, S>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
        Mode::Provider: EventWriter<Mode> + 'static,
    {
        self.validate_config();

        let layer = self.build_layer();

        let filter = self.build_filter(layer.provider.clone());

        let targets = self.build_target_filter(target);

        layer.with_filter(filter.and(targets))
    }

    #[cfg(feature = "global_filter")]
    pub fn build<S>(self) -> EtwLayer<S, Mode::Provider>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
        Mode::Provider: EventWriter + 'static,
    {
        self.validate_config();

        self.build_layer()
    }

    #[allow(clippy::type_complexity)]
    #[cfg(not(feature = "global_filter"))]
    pub fn build<S>(self) -> Filtered<EtwLayer<S, Mode>, EtwFilter<S, Mode>, S>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
        Mode::Provider: EventWriter<Mode> + 'static,
    {
        self.validate_config();

        let layer = self.build_layer();

        let filter = self.build_filter(layer.provider.clone());

        layer.with_filter(filter)
    }
}

#[cfg(not(feature = "global_filter"))]
impl<S, Mode> Filter<S> for EtwFilter<S, Mode>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    Mode: ProviderTypes + 'static,
    Mode::Provider: EventWriter<Mode> + 'static,
{
    fn callsite_enabled(
        &self,
        metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        let etw_meta = EVENT_METADATA.get(&metadata.callsite());
        let keyword = if let Some(meta) = etw_meta {
            meta.kw
        } else {
            self.default_keyword
        };

        if Mode::supports_enable_callback() {
            if self.provider.enabled(map_level(metadata.level()), keyword) {
                tracing::subscriber::Interest::always()
            } else {
                tracing::subscriber::Interest::never()
            }
        } else {
            // Returning "sometimes" means the enabled function will be called every time an event or span is created from the callsite.
            // This will let us perform a global "is enabled" check each time.
            tracing::subscriber::Interest::sometimes()
        }
    }

    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        _cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        let etw_meta = EVENT_METADATA.get(&metadata.callsite());
        let keyword = if let Some(meta) = etw_meta {
            meta.kw
        } else {
            self.default_keyword
        };

        self.provider
            .enabled(map_level(metadata.level()), keyword)
    }

    fn event_enabled(
        &self,
        event: &tracing::Event<'_>,
        _cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        let etw_meta = EVENT_METADATA.get(&event.metadata().callsite());
        let keyword = if let Some(meta) = etw_meta {
            meta.kw
        } else {
            self.default_keyword
        };

        self.provider
            .enabled(map_level(event.metadata().level()), keyword)
    }
}

struct SpanData {
    fields: Box<[FieldValueIndex]>,
    activity_id: [u8; 16], // // if set, byte 0 is 1 and 64-bit span ID in the lower 8 bytes
    related_activity_id: [u8; 16], // if set, byte 0 is 1 and 64-bit span ID in the lower 8 bytes
    start_time: SystemTime,
}

impl<S, Mode> Layer<S> for EtwLayer<S, Mode>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    Mode: ProviderTypes + 'static,
    Mode::Provider: EventWriter<Mode> + 'static,
{
    fn on_register_dispatch(&self, _collector: &tracing::Dispatch) {
        // Late init when the layer is installed as a subscriber
    }

    fn on_layer(&mut self, _subscriber: &mut S) {
        // Late init when the layer is attached to a subscriber
    }

    #[cfg(feature = "global_filter")]
    fn register_callsite(
        &self,
        metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        let etw_meta = EVENT_METADATA.get(&metadata.callsite());
        let keyword = if let Some(meta) = etw_meta {
            meta.kw
        } else {
            self.default_keyword
        };

        if P::supports_enable_callback() {
            if self.provider.enabled(map_level(metadata.level()), keyword) {
                tracing::subscriber::Interest::always()
            } else {
                tracing::subscriber::Interest::never()
            }
        } else {
            // Returning "sometimes" means the enabled function will be called every time an event or span is created from the callsite.
            // This will let us perform a global "is enabled" check each time.
            tracing::subscriber::Interest::sometimes()
        }
    }

    #[cfg(feature = "global_filter")]
    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        let etw_meta = EVENT_METADATA.get(&metadata.callsite());
        let keyword = if let Some(meta) = etw_meta {
            meta.kw
        } else {
            self.default_keyword
        };

        self.provider.enabled(map_level(metadata.level()), keyword)
    }

    #[cfg(feature = "global_filter")]
    fn event_enabled(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        let etw_meta = EVENT_METADATA.get(&event.metadata().callsite());
        let keyword = if let Some(meta) = etw_meta {
            meta.kw
        } else {
            self.default_keyword
        };

        self.provider
            .enabled(map_level(event.metadata().level()), keyword)
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let timestamp = std::time::SystemTime::now();

        let current_span = ctx
            .event_span(event)
            .map(|evt| evt.id())
            .map_or(0, |id| (id.into_u64()));
        let parent_span = ctx
            .event_span(event)
            .map_or(0, |evt| evt.parent().map_or(0, |p| p.id().into_u64()));

        let etw_meta = EVENT_METADATA.get(&event.metadata().callsite());
        let (name, keyword, tag) = if let Some(meta) = etw_meta {
            (event.metadata().name(), meta.kw, meta.event_tag)
        } else {
            (event.metadata().name(), self.default_keyword, 0)
        };

        self.provider.as_ref().write_record(
            timestamp,
            current_span,
            parent_span,
            name,
            map_level(event.metadata().level()),
            keyword,
            tag,
            event,
        );
    }

    fn on_new_span(
        &self,
        attrs: &span::Attributes<'_>,
        id: &span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let span = if let Some(span) = ctx.span(id) {
            span
        } else {
            return;
        };

        if span.extensions().get::<SpanData>().is_some() {
            return;
        }

        let metadata = span.metadata();

        let parent_span_id = if attrs.is_contextual() {
            attrs.parent().map_or(0, |id| id.into_u64())
        } else {
            0
        };

        let n = metadata.fields().len();

        let mut data = {
            let mut v: Vec<FieldValueIndex> = Vec::with_capacity(n);
            v.resize_with(n, Default::default);

            let mut i = 0;
            for field in metadata.fields().iter() {
                v[i].field = field.name();
                v[i].value = ValueTypes::None;
                v[i].sort_index = i as u8;
                i += 1;
            }

            let mut indexes: [u8; 32] = [
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
                23, 24, 25, 26, 27, 28, 29, 30, 31,
            ];

            indexes[0..n].sort_by_key(|idx| v[v[*idx as usize].sort_index as usize].field);

            i = 0;
            for f in &mut v {
                f.sort_index = indexes[i];
                i += 1;
            }

            SpanData {
                fields: v.into_boxed_slice(),
                activity_id: *GLOBAL_ACTIVITY_SEED,
                related_activity_id: *GLOBAL_ACTIVITY_SEED,
                start_time: SystemTime::UNIX_EPOCH,
            }
        };

        let (_, half) = data.activity_id.split_at_mut(8);
        half.copy_from_slice(&id.into_u64().to_le_bytes());

        data.activity_id[0] = 1;
        data.related_activity_id[0] = if parent_span_id != 0 {
            let (_, half) = data.related_activity_id.split_at_mut(8);
            half.copy_from_slice(&parent_span_id.to_le_bytes());
            1
        } else {
            0
        };

        attrs.values().record(&mut ValueVisitor {
            fields: &mut data.fields,
        });

        // This will unfortunately box data. It would be ideal if we could avoid this second heap allocation
        // by packing everything into a single alloc.
        span.extensions_mut().replace(data);
    }

    fn on_enter(&self, id: &span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        // A span was started
        let timestamp = std::time::SystemTime::now();

        let span = if let Some(span) = ctx.span(id) {
            span
        } else {
            return;
        };

        let metadata = span.metadata();

        let mut extensions = span.extensions_mut();
        let data = if let Some(data) = extensions.get_mut::<SpanData>() {
            data
        } else {
            // We got a span that was entered without being new'ed?
            return;
        };

        let etw_meta = EVENT_METADATA.get(&metadata.callsite());
        let (keyword, tag) = if let Some(meta) = etw_meta {
            (meta.kw, meta.event_tag)
        } else {
            (self.default_keyword, 0)
        };

        self.provider.as_ref().span_start(
            &span,
            timestamp,
            &data.activity_id,
            &data.related_activity_id,
            &data.fields,
            map_level(metadata.level()),
            keyword,
            tag,
        );

        data.start_time = timestamp;
    }

    fn on_exit(&self, id: &span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        // A span was exited
        let stop_timestamp = std::time::SystemTime::now();

        let span = if let Some(span) = ctx.span(id) {
            span
        } else {
            return;
        };

        let metadata = span.metadata();

        let mut extensions = span.extensions_mut();
        let data = if let Some(data) = extensions.get_mut::<SpanData>() {
            data
        } else {
            // We got a span that was entered without being new'ed?
            return;
        };

        let etw_meta = EVENT_METADATA.get(&metadata.callsite());
        let (keyword, tag) = if let Some(meta) = etw_meta {
            (meta.kw, meta.event_tag)
        } else {
            (self.default_keyword, 0)
        };

        self.provider.as_ref().span_stop(
            &span,
            (data.start_time, stop_timestamp),
            &data.activity_id,
            &data.related_activity_id,
            &data.fields,
            map_level(metadata.level()),
            keyword,
            tag,
        );
    }

    fn on_close(&self, _id: span::Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        // A span was closed
        // Good for knowing when to log a summary event?
    }

    fn on_record(
        &self,
        id: &span::Id,
        values: &span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // Values were added to the given span

        let span = if let Some(span) = ctx.span(id) {
            span
        } else {
            return;
        };

        let mut extensions = span.extensions_mut();
        let data = if let Some(data) = extensions.get_mut::<SpanData>() {
            data
        } else {
            // We got a span that was entered without being new'ed?
            return;
        };

        values.record(&mut ValueVisitor {
            fields: &mut data.fields,
        });
    }
}
