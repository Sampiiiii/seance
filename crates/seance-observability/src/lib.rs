// Owns shared tracing vocabulary and scoped timing helpers for performance instrumentation.

use std::time::Instant;

use tracing::{Level, span::EnteredSpan, trace, trace_span};

pub const RENDER_TRACE_TARGET: &str = "seance_render";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderDomain {
    Ui,
    Terminal,
}

impl RenderDomain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Terminal => "terminal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderPath {
    Frame,
    TerminalPane,
    TerminalRefreshRequest,
    TerminalGeometryRefresh,
    TerminalSurfaceSync,
    TerminalRowPaintTemplate,
    TerminalFragmentPlan,
}

impl RenderPath {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Frame => "frame",
            Self::TerminalPane => "terminal_pane",
            Self::TerminalRefreshRequest => "terminal_refresh_request",
            Self::TerminalGeometryRefresh => "terminal_geometry_refresh",
            Self::TerminalSurfaceSync => "terminal_surface_sync",
            Self::TerminalRowPaintTemplate => "terminal_row_paint_template",
            Self::TerminalFragmentPlan => "terminal_fragment_plan",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderCause {
    DisplayProbe,
    Input,
    TerminalUpdate,
    Palette,
    UiRefresh,
    #[default]
    Unknown,
}

impl RenderCause {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DisplayProbe => "probe",
            Self::Input => "input",
            Self::TerminalUpdate => "terminal",
            Self::Palette => "palette",
            Self::UiRefresh => "ui",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderPhase {
    Compose,
    Schedule,
    Apply,
    Reconcile,
    FastPath,
    IterateRows,
    CacheLookup,
    Build,
    Shape,
    Summary,
}

impl RenderPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compose => "compose",
            Self::Schedule => "schedule",
            Self::Apply => "apply",
            Self::Reconcile => "reconcile",
            Self::FastPath => "fast_path",
            Self::IterateRows => "iterate_rows",
            Self::CacheLookup => "cache_lookup",
            Self::Build => "build",
            Self::Shape => "shape",
            Self::Summary => "summary",
        }
    }
}

#[must_use]
pub struct RenderTraceScope {
    domain: RenderDomain,
    path: RenderPath,
    cause: RenderCause,
    started_at: Option<Instant>,
    entered_span: Option<EnteredSpan>,
}

impl RenderTraceScope {
    pub fn new(domain: RenderDomain, path: RenderPath, cause: RenderCause) -> Self {
        if !render_trace_enabled() {
            return Self {
                domain,
                path,
                cause,
                started_at: None,
                entered_span: None,
            };
        }

        let span = trace_span!(
            target: RENDER_TRACE_TARGET,
            "render_scope",
            render_domain = domain.as_str(),
            render_path = path.as_str(),
            render_cause = cause.as_str(),
        );

        trace!(
            target: RENDER_TRACE_TARGET,
            render_domain = domain.as_str(),
            render_path = path.as_str(),
            render_cause = cause.as_str(),
            "render scope start"
        );

        Self {
            domain,
            path,
            cause,
            started_at: Some(Instant::now()),
            entered_span: Some(span.entered()),
        }
    }

    pub fn phase(&self, phase: RenderPhase) -> RenderPhaseScope {
        if self.started_at.is_none() {
            return RenderPhaseScope::disabled(self.domain, self.path, self.cause, phase);
        }

        let span = trace_span!(
            target: RENDER_TRACE_TARGET,
            "render_phase",
            render_domain = self.domain.as_str(),
            render_path = self.path.as_str(),
            render_cause = self.cause.as_str(),
            render_phase = phase.as_str(),
        );

        trace!(
            target: RENDER_TRACE_TARGET,
            render_domain = self.domain.as_str(),
            render_path = self.path.as_str(),
            render_cause = self.cause.as_str(),
            render_phase = phase.as_str(),
            "render phase start"
        );

        RenderPhaseScope {
            domain: self.domain,
            path: self.path,
            cause: self.cause,
            phase,
            started_at: Some(Instant::now()),
            entered_span: Some(span.entered()),
        }
    }
}

impl Drop for RenderTraceScope {
    fn drop(&mut self) {
        let Some(started_at) = self.started_at.take() else {
            return;
        };

        trace!(
            target: RENDER_TRACE_TARGET,
            render_domain = self.domain.as_str(),
            render_path = self.path.as_str(),
            render_cause = self.cause.as_str(),
            elapsed_ms = elapsed_ms(started_at),
            "render scope complete"
        );
        self.entered_span.take();
    }
}

#[must_use]
pub struct RenderPhaseScope {
    domain: RenderDomain,
    path: RenderPath,
    cause: RenderCause,
    phase: RenderPhase,
    started_at: Option<Instant>,
    entered_span: Option<EnteredSpan>,
}

impl RenderPhaseScope {
    fn disabled(
        domain: RenderDomain,
        path: RenderPath,
        cause: RenderCause,
        phase: RenderPhase,
    ) -> Self {
        Self {
            domain,
            path,
            cause,
            phase,
            started_at: None,
            entered_span: None,
        }
    }
}

impl Drop for RenderPhaseScope {
    fn drop(&mut self) {
        let Some(started_at) = self.started_at.take() else {
            return;
        };

        trace!(
            target: RENDER_TRACE_TARGET,
            render_domain = self.domain.as_str(),
            render_path = self.path.as_str(),
            render_cause = self.cause.as_str(),
            render_phase = self.phase.as_str(),
            elapsed_ms = elapsed_ms(started_at),
            "render phase complete"
        );
        self.entered_span.take();
    }
}

pub fn render_trace_enabled() -> bool {
    tracing::enabled!(target: RENDER_TRACE_TARGET, Level::TRACE)
}

fn elapsed_ms(started_at: Instant) -> f64 {
    started_at.elapsed().as_secs_f64() * 1_000.0
}
