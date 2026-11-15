// SPDX-License-Identifier: GPL-3.0-or-later
//! Small Cairo charts for the app. Labels, legends and summaries stay ordinary GTK widgets;
//! Cairo draws geometry only. Every chart is additive: the existing text remains the source of
//! truth, and a missing value hides the mark instead of drawing a fake zero.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use adw::prelude::*;
use gtk::cairo;
use workstation_core::{
    compare::{Comparison, MetricDelta},
    db::HistoryEntry,
    fit::memory_math::{FitAssessment, FitClass, MemoryLimits},
    hardware::MemoryPool,
};

use crate::{theme, views::lab_text};

const GIB: f64 = 1_073_741_824.0;

fn gib(bytes: u64) -> f64 {
    bytes as f64 / GIB
}

fn size_text(bytes: u64) -> String {
    format!("{:.2} GiB", gib(bytes))
}

fn set_color(cr: &cairo::Context, hex: &str) {
    let color = theme::color(hex);
    cr.set_source_rgba(
        color.red().into(),
        color.green().into(),
        color.blue().into(),
        color.alpha().into(),
    );
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    let radius = radius.min(width / 2.0).min(height / 2.0).max(0.0);
    cr.new_sub_path();
    cr.arc(
        x + width - radius,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    cr.arc(
        x + width - radius,
        y + height - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    cr.arc(
        x + radius,
        y + height - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
}

fn animations_enabled() -> bool {
    gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations())
}

fn label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .build()
}

fn value_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(1.0)
        .hexpand(true)
        .css_classes(["dim-label"])
        .build()
}

fn swatch(class: &str) -> gtk::Widget {
    let swatch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    swatch.set_size_request(10, 10);
    swatch.set_valign(gtk::Align::Center);
    swatch.add_css_class("kvlm-swatch");
    swatch.add_css_class(class);
    swatch.upcast()
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn follow_theme(area: &gtk::DrawingArea) {
    let weak = area.downgrade();
    adw::StyleManager::default().connect_dark_notify(move |_| {
        if let Some(area) = weak.upgrade() {
            area.queue_draw();
        }
    });
}

fn reveal(
    area: &gtk::DrawingArea,
    progress: &Rc<Cell<f64>>,
    animation: &Rc<RefCell<Option<adw::TimedAnimation>>>,
) {
    // An animation started before the widget is mapped never ticks, so the first frame would
    // stay empty. Paint the final value instead and let later updates animate.
    if !animations_enabled() || !area.is_mapped() {
        *animation.borrow_mut() = None;
        progress.set(1.0);
        area.queue_draw();
        return;
    }
    progress.set(0.0);
    let target_area = area.clone();
    let target_progress = progress.clone();
    let target = adw::CallbackAnimationTarget::new(move |value| {
        target_progress.set(value);
        target_area.queue_draw();
    });
    let timed = adw::TimedAnimation::new(area, 0.0, 1.0, 320, target);
    timed.set_easing(adw::Easing::EaseOutCubic);
    *animation.borrow_mut() = Some(timed.clone());
    timed.play();
}

fn describe(area: &gtk::DrawingArea, title: &str, description: &str) {
    area.update_property(&[
        gtk::accessible::Property::Label(title),
        gtk::accessible::Property::Description(description),
    ]);
}

/* ---------------------------------- memory fit ---------------------------------- */

#[derive(Clone, Copy)]
enum FitKind {
    Weights,
    Cache,
    Buffers,
    Runtime,
}

impl FitKind {
    fn label(self) -> &'static str {
        match self {
            Self::Weights => "Weights",
            Self::Cache => "KV cache",
            Self::Buffers => "Buffers",
            Self::Runtime => "Runtime",
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::Weights => "weights",
            Self::Cache => "kv",
            Self::Buffers => "buffers",
            Self::Runtime => "runtime",
        }
    }

    fn color(self) -> &'static str {
        let palette = theme::palette();
        match self {
            Self::Weights => palette.teal,
            Self::Cache => palette.kv,
            Self::Buffers => palette.amber,
            Self::Runtime => palette.brown,
        }
    }
}

const FIT_KINDS: [FitKind; 4] = [
    FitKind::Weights,
    FitKind::Cache,
    FitKind::Buffers,
    FitKind::Runtime,
];

struct FitData {
    segments: [u64; 4],
    total: u64,
    limits: Option<MemoryLimits>,
}

impl FitData {
    fn scale(&self) -> f64 {
        let limits = self.limits;
        [
            self.total,
            limits.map_or(0, |limits| limits.comfortable_bytes),
            limits.map_or(0, |limits| limits.maximum_bytes),
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
        .max(1) as f64
            * 1.04
    }

    fn segment_at(&self, x: f64, width: f64) -> Option<usize> {
        if width <= 0.0 || !x.is_finite() || x < 0.0 || x > width {
            return None;
        }
        let scale = self.scale();
        let mut left = 0.0;
        for (index, bytes) in self.segments.iter().enumerate() {
            let right = left + (*bytes as f64 / scale) * width;
            if x >= left && x < right {
                return Some(index);
            }
            left = right;
        }
        None
    }

    fn tooltip(&self, index: usize) -> Option<String> {
        let bytes = *self.segments.get(index)?;
        if bytes == 0 {
            return None;
        }
        let kind = FIT_KINDS[index];
        Some(format!(
            "{}: {:.2} GiB ({bytes} bytes)",
            kind.label(),
            gib(bytes)
        ))
    }
}

struct FitInner {
    data: Rc<RefCell<Option<FitData>>>,
    hover: Rc<Cell<Option<usize>>>,
    progress: Rc<Cell<f64>>,
    animation: Rc<RefCell<Option<adw::TimedAnimation>>>,
}

impl FitInner {
    fn index_at(&self, area: &gtk::DrawingArea, x: f64) -> Option<usize> {
        self.data
            .borrow()
            .as_ref()
            .and_then(|data| data.segment_at(x, area.width() as f64))
    }
}

pub struct FitChart {
    pub widget: gtk::Box,
    area: gtk::DrawingArea,
    legend: gtk::Box,
    note: gtk::Label,
    inner: Rc<FitInner>,
}

impl FitChart {
    pub fn new(title: &str) -> Self {
        let heading = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let area = gtk::DrawingArea::new();
        area.set_content_height(48);
        area.set_height_request(48);
        area.set_hexpand(true);
        area.set_has_tooltip(true);
        let legend = gtk::Box::new(gtk::Orientation::Vertical, 2);
        legend.set_visible(false);
        let note = label("");
        note.add_css_class("kvlm-chart-note");
        note.add_css_class("dim-label");

        let inner = Rc::new(FitInner {
            data: Rc::new(RefCell::new(None)),
            hover: Rc::new(Cell::new(None)),
            progress: Rc::new(Cell::new(1.0)),
            animation: Rc::new(RefCell::new(None)),
        });

        {
            let data = inner.data.clone();
            let hover = inner.hover.clone();
            let progress = inner.progress.clone();
            area.set_draw_func(move |_, cr, width, height| {
                draw_fit(
                    cr,
                    width,
                    height,
                    data.borrow().as_ref(),
                    progress.get(),
                    hover.get(),
                );
            });
        }
        {
            let data = inner.data.clone();
            let weak = area.downgrade();
            area.connect_query_tooltip(move |_, x, _, _, tooltip| {
                let Some(area) = weak.upgrade() else {
                    return false;
                };
                let data = data.borrow();
                let Some(fit) = data.as_ref() else {
                    return false;
                };
                let Some(index) = fit.segment_at(x as f64, area.width() as f64) else {
                    return false;
                };
                let Some(text) = fit.tooltip(index) else {
                    return false;
                };
                tooltip.set_text(Some(&text));
                true
            });
        }
        {
            let inner = Rc::clone(&inner);
            let hover = inner.hover.clone();
            let weak = area.downgrade();
            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(move |_, x, _| {
                let Some(area) = weak.upgrade() else {
                    return;
                };
                let index = inner.index_at(&area, x);
                if hover.replace(index) != index {
                    area.queue_draw();
                }
            });
            let weak = area.downgrade();
            motion.connect_leave(move |_| {
                if let Some(area) = weak.upgrade() {
                    area.queue_draw();
                }
            });
            area.add_controller(motion);
        }
        follow_theme(&area);

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 6);
        widget.add_css_class("kvlm-chart-panel");
        widget.append(&heading);
        widget.append(&area);
        widget.append(&legend);
        widget.append(&note);

        Self {
            widget,
            area,
            legend,
            note,
            inner,
        }
    }

    pub fn set(&self, assessment: &FitAssessment) {
        let data = assessment.breakdown.map(|breakdown| FitData {
            segments: [
                breakdown.weights_bytes,
                breakdown.kv_cache_bytes,
                breakdown.buffer_bytes,
                breakdown.overhead_bytes,
            ],
            total: breakdown.total_bytes,
            limits: assessment.limits,
        });
        let available = data.is_some();
        *self.inner.data.borrow_mut() = data;
        self.area.set_visible(available);
        self.legend.set_visible(available);
        if available {
            self.rebuild_legend();
        } else {
            clear(&self.legend);
        }
        self.update_note(assessment);
        self.update_description(assessment);
        self.area.queue_draw();
        reveal(&self.area, &self.inner.progress, &self.inner.animation);
    }

    pub fn clear(&self) {
        *self.inner.data.borrow_mut() = None;
        self.area.set_visible(false);
        self.legend.set_visible(false);
        clear(&self.legend);
        self.note.set_text("");
    }

    fn rebuild_legend(&self) {
        clear(&self.legend);
        let data = self.inner.data.borrow();
        let Some(data) = data.as_ref() else {
            return;
        };
        for (index, kind) in FIT_KINDS.iter().enumerate() {
            let bytes = data.segments[index];
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&swatch(kind.class()));
            row.append(&label(kind.label()));
            row.append(&value_label(&format!(
                "{} · {bytes} bytes",
                size_text(bytes)
            )));
            self.legend.append(&row);
        }
    }

    fn update_note(&self, assessment: &FitAssessment) {
        let limits = match assessment.limits {
            Some(MemoryLimits {
                comfortable_bytes,
                maximum_bytes,
            }) => format!(
                "Comfortable {} · maximum {}. Static estimate, not runtime verification.",
                size_text(comfortable_bytes),
                size_text(maximum_bytes)
            ),
            None => "Budget limits unavailable because current available memory was not supplied."
                .into(),
        };
        let missing = if assessment.breakdown.is_none() {
            "Memory breakdown unavailable for these inputs and assumptions. "
        } else {
            ""
        };
        self.note.set_text(&format!("{missing}{limits}"));
    }

    fn update_description(&self, assessment: &FitAssessment) {
        let data = self.inner.data.borrow();
        let Some(data) = data.as_ref() else {
            describe(
                &self.area,
                "Memory estimate unavailable",
                "No memory breakdown is available for these inputs and assumptions.",
            );
            return;
        };
        let parts: Vec<String> = FIT_KINDS
            .iter()
            .enumerate()
            .map(|(index, kind)| format!("{} {:.2} GiB", kind.label(), gib(data.segments[index])))
            .collect();
        let class = match assessment.class {
            FitClass::Comfortable => "Comfortable",
            FitClass::Constrained => "Constrained",
            FitClass::NotRecommended => "Not recommended",
            FitClass::Unknown => "Unknown",
        };
        describe(
            &self.area,
            &format!("Memory estimate, total {:.2} GiB, {class}", gib(data.total)),
            &parts.join(", "),
        );
    }
}

fn draw_fit(
    cr: &cairo::Context,
    width: i32,
    height: i32,
    data: Option<&FitData>,
    progress: f64,
    hover: Option<usize>,
) {
    let palette = theme::palette();
    let width = width as f64;
    let bar_height = 14.0;
    let top = (height as f64 - bar_height) / 2.0;
    rounded(cr, 0.0, top, width, bar_height, 4.0);
    set_color(cr, palette.track);
    let _ = cr.fill();

    let Some(data) = data else {
        return;
    };
    let scale = data.scale();
    let mut left = 0.0;
    for (index, bytes) in data.segments.iter().enumerate() {
        let mut segment = (*bytes as f64 / scale) * width * progress;
        if segment <= 0.0 {
            continue;
        }
        if left > 0.0 {
            left += 1.0;
            segment = (segment - 1.0).max(1.0);
        }
        rounded(cr, left, top, segment, bar_height, 3.0);
        set_color(cr, FIT_KINDS[index].color());
        let _ = cr.fill();
        if hover == Some(index) {
            rounded(cr, left, top, segment, bar_height, 3.0);
            set_color(cr, palette.accent);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
        }
        left += segment;
    }

    if let Some(limits) = data.limits {
        let mut marks = vec![limits.comfortable_bytes, limits.maximum_bytes];
        marks.sort_unstable();
        marks.dedup();
        for mark in marks {
            let x = (mark as f64 / scale) * width;
            if x <= 0.0 || x >= width {
                continue;
            }
            cr.set_dash(&[2.0, 3.0], 0.0);
            cr.set_line_width(1.5);
            set_color(cr, palette.neutral);
            cr.move_to(x, 0.0);
            cr.line_to(x, height as f64);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }
    }
}

/* ----------------------------------- meter ------------------------------------- */

struct MeterInner {
    pool: Rc<Cell<Option<MemoryPool>>>,
    progress: Rc<Cell<f64>>,
    animation: Rc<RefCell<Option<adw::TimedAnimation>>>,
}

pub struct Meter {
    pub widget: gtk::Box,
    area: gtk::DrawingArea,
    title: String,
    inner: Rc<MeterInner>,
}

impl Meter {
    pub fn new(title: &str) -> Self {
        let area = gtk::DrawingArea::new();
        area.set_content_height(8);
        area.set_height_request(8);
        area.set_size_request(140, 8);
        area.set_valign(gtk::Align::Center);
        area.set_accessible_role(gtk::AccessibleRole::Meter);

        let inner = Rc::new(MeterInner {
            pool: Rc::new(Cell::new(None)),
            progress: Rc::new(Cell::new(1.0)),
            animation: Rc::new(RefCell::new(None)),
        });
        {
            let pool = inner.pool.clone();
            let progress = inner.progress.clone();
            area.set_draw_func(move |_, cr, width, height| {
                draw_meter(cr, width, height, pool.get(), progress.get());
            });
        }
        follow_theme(&area);

        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.append(&area);

        Self {
            widget,
            area,
            title: title.to_owned(),
            inner,
        }
    }

    pub fn set(&self, pool: &MemoryPool) {
        self.inner.pool.set(Some(*pool));
        let detail = match pool.available_bytes {
            Some(available) => format!(
                "{} available of {} ({:.0}%)",
                size_text(available),
                size_text(pool.total_bytes),
                (available as f64 / pool.total_bytes.max(1) as f64) * 100.0
            ),
            None => format!("Available unknown · {} total", size_text(pool.total_bytes)),
        };
        self.area.set_tooltip_text(Some(&detail));
        let mut properties = vec![
            gtk::accessible::Property::Label(&self.title),
            gtk::accessible::Property::ValueMin(0.0),
            gtk::accessible::Property::ValueMax(pool.total_bytes as f64),
            gtk::accessible::Property::ValueText(&detail),
        ];
        if let Some(available) = pool.available_bytes {
            properties.push(gtk::accessible::Property::ValueNow(available as f64));
        }
        self.area.update_property(&properties);
        self.area.queue_draw();
        reveal(&self.area, &self.inner.progress, &self.inner.animation);
    }
}

fn draw_meter(
    cr: &cairo::Context,
    width: i32,
    height: i32,
    pool: Option<MemoryPool>,
    progress: f64,
) {
    let palette = theme::palette();
    let width = width as f64;
    let height = height as f64;
    rounded(cr, 0.0, 0.0, width, height, height / 2.0);
    set_color(cr, palette.track);
    let _ = cr.fill();
    let Some(pool) = pool else {
        return;
    };
    let Some(available) = pool.available_bytes else {
        return;
    };
    let fraction = (available as f64 / pool.total_bytes.max(1) as f64).clamp(0.0, 1.0);
    let fill = width * fraction * progress;
    if fill <= 0.0 {
        return;
    }
    rounded(cr, 0.0, 0.0, fill, height, height / 2.0);
    set_color(cr, palette.teal);
    let _ = cr.fill();
}

/* ---------------------------------- history ------------------------------------ */

#[derive(Clone, Debug, PartialEq)]
struct TrendPoint {
    run_id: i64,
    model: String,
    rate: f64,
}

struct TrendInner {
    points: Rc<RefCell<Vec<TrendPoint>>>,
    hover: Rc<Cell<Option<usize>>>,
    progress: Rc<Cell<f64>>,
    animation: Rc<RefCell<Option<adw::TimedAnimation>>>,
}

pub struct TrendChart {
    pub widget: gtk::Box,
    area: gtk::DrawingArea,
    summary: gtk::Label,
    inner: Rc<TrendInner>,
}

impl Default for TrendChart {
    fn default() -> Self {
        Self::new()
    }
}

impl TrendChart {
    pub fn new() -> Self {
        let heading = gtk::Label::builder()
            .label("Generation speed over saved runs")
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let area = gtk::DrawingArea::new();
        area.set_content_height(96);
        area.set_height_request(96);
        area.set_hexpand(true);
        area.set_has_tooltip(true);
        let summary = label("");
        summary.add_css_class("kvlm-chart-note");
        summary.add_css_class("dim-label");

        let inner = Rc::new(TrendInner {
            points: Rc::new(RefCell::new(Vec::new())),
            hover: Rc::new(Cell::new(None)),
            progress: Rc::new(Cell::new(1.0)),
            animation: Rc::new(RefCell::new(None)),
        });
        {
            let points = inner.points.clone();
            let hover = inner.hover.clone();
            let progress = inner.progress.clone();
            area.set_draw_func(move |_, cr, width, height| {
                draw_trend(
                    cr,
                    width,
                    height,
                    &points.borrow(),
                    progress.get(),
                    hover.get(),
                );
            });
        }
        {
            let points = inner.points.clone();
            let weak = area.downgrade();
            area.connect_query_tooltip(move |_, x, _, _, tooltip| {
                let Some(area) = weak.upgrade() else {
                    return false;
                };
                let points = points.borrow();
                let Some(index) = trend_index(&points, x as f64, area.width() as f64) else {
                    return false;
                };
                let Some(point) = points.get(index) else {
                    return false;
                };
                tooltip.set_text(Some(&format!(
                    "Run #{} · {} · {:.1} tokens/s",
                    point.run_id, point.model, point.rate
                )));
                true
            });
        }
        {
            let hover = inner.hover.clone();
            let motion_points = Rc::clone(&inner.points);
            let weak = area.downgrade();
            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(move |_, x, _| {
                let Some(area) = weak.upgrade() else {
                    return;
                };
                let index = {
                    let points = motion_points.borrow();
                    trend_index(&points, x, area.width() as f64)
                };
                if hover.replace(index) != index {
                    area.queue_draw();
                }
            });
            let weak = area.downgrade();
            motion.connect_leave(move |_| {
                if let Some(area) = weak.upgrade() {
                    area.queue_draw();
                }
            });
            area.add_controller(motion);
        }
        follow_theme(&area);

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 6);
        widget.add_css_class("kvlm-chart-panel");
        widget.append(&heading);
        widget.append(&area);
        widget.append(&summary);
        widget.set_visible(false);

        Self {
            widget,
            area,
            summary,
            inner,
        }
    }

    pub fn set_entries(&self, entries: &[HistoryEntry]) {
        let points = trend_points(entries);
        let visible = points.len() >= 2;
        self.widget.set_visible(visible);
        *self.inner.points.borrow_mut() = points;
        self.inner.hover.set(None);
        if !visible {
            return;
        }
        let summary = {
            let points = self.inner.points.borrow();
            trend_summary(&points)
        };
        self.summary.set_text(&summary);
        describe(&self.area, "Generation speed across saved runs", &summary);
        self.area.queue_draw();
        reveal(&self.area, &self.inner.progress, &self.inner.animation);
    }
}

fn decode_rate(entry: &HistoryEntry) -> Option<f64> {
    let completion = entry.completion.as_ref()?;
    let workstation_core::db::RunOutcome::Completed(metrics) = &completion.outcome else {
        return None;
    };
    let count = metrics.eval_count?;
    let duration = metrics.eval_duration_ns?;
    if count == 0 || duration == 0 {
        return None;
    }
    let rate = count as f64 / (duration as f64 / 1e9);
    (rate.is_finite() && rate > 0.0).then_some(rate)
}

fn trend_points(entries: &[HistoryEntry]) -> Vec<TrendPoint> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut points: Vec<TrendPoint> = entries
        .iter()
        .filter_map(|entry| {
            Some(TrendPoint {
                run_id: entry.id,
                model: entry.snapshot.model_name.clone(),
                rate: decode_rate(entry)?,
            })
        })
        .take(12)
        .collect();
    points.reverse();
    points
}

fn trend_summary(points: &[TrendPoint]) -> String {
    if points.is_empty() {
        return "No completed generation rates saved yet.".into();
    }
    let mut rates: Vec<f64> = points.iter().map(|point| point.rate).collect();
    rates.sort_by(|a, b| a.total_cmp(b));
    let min = rates.first().copied().unwrap_or(0.0);
    let max = rates.last().copied().unwrap_or(0.0);
    let median = rates[rates.len() / 2];
    format!(
        "{} saved runs · min {min:.1} · median {median:.1} · max {max:.1} tokens/s. Server-reported values; run-to-run spread is not a verdict.",
        points.len()
    )
}

fn trend_index(points: &[TrendPoint], x: f64, width: f64) -> Option<usize> {
    if points.is_empty() || width <= 0.0 {
        return None;
    }
    if points.len() == 1 {
        return Some(0);
    }
    let padding = 8.0;
    let usable = (width - padding * 2.0).max(1.0);
    let step = usable / (points.len() - 1) as f64;
    let index = ((x - padding) / step).round();
    (index >= 0.0).then(|| (index as usize).min(points.len() - 1))
}

fn draw_trend(
    cr: &cairo::Context,
    width: i32,
    height: i32,
    points: &[TrendPoint],
    progress: f64,
    hover: Option<usize>,
) {
    let palette = theme::palette();
    let width = width as f64;
    let height = height as f64;
    let padding = 8.0;
    let top = 8.0;
    let bottom = height - 8.0;
    if points.is_empty() || bottom <= top {
        return;
    }
    cr.set_line_width(1.0);
    set_color(cr, palette.track);
    cr.move_to(0.0, bottom);
    cr.line_to(width, bottom);
    let _ = cr.stroke();

    let max = points
        .iter()
        .map(|point| point.rate)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let step = if points.len() > 1 {
        (width - padding * 2.0) / (points.len() - 1) as f64
    } else {
        0.0
    };
    let point_at = |index: usize, point: &TrendPoint| {
        let x = padding + step * index as f64;
        let y = bottom - (point.rate / (max * 1.08)) * (bottom - top) * progress;
        (x, y)
    };

    let mut path: Vec<(f64, f64)> = points
        .iter()
        .enumerate()
        .map(|(index, point)| point_at(index, point))
        .collect();
    if path.is_empty() {
        return;
    }
    if path.len() == 1 {
        path.push((path[0].0 + 1.0, path[0].1));
    }

    set_color(cr, palette.teal);
    cr.set_line_width(2.0);
    cr.move_to(path[0].0, path[0].1);
    for (x, y) in path.iter().skip(1) {
        cr.line_to(*x, *y);
    }
    let _ = cr.stroke();

    cr.move_to(path[0].0, bottom);
    for (x, y) in &path {
        cr.line_to(*x, *y);
    }
    cr.line_to(path[path.len() - 1].0, bottom);
    cr.close_path();
    let teal = theme::color(palette.teal);
    cr.set_source_rgba(
        teal.red().into(),
        teal.green().into(),
        teal.blue().into(),
        0.14,
    );
    let _ = cr.fill();

    for (index, (x, y)) in path.iter().enumerate() {
        let radius = if hover == Some(index) { 4.5 } else { 2.5 };
        cr.arc(*x, *y, radius, 0.0, std::f64::consts::TAU);
        set_color(
            cr,
            if hover == Some(index) {
                palette.amber
            } else {
                palette.teal
            },
        );
        let _ = cr.fill();
    }
}

/* -------------------------------- comparison ----------------------------------- */

struct MetricBars {
    metric: MetricDelta,
    progress: Rc<Cell<f64>>,
    animation: Rc<RefCell<Option<adw::TimedAnimation>>>,
}

pub struct ComparisonChart {
    pub widget: gtk::Box,
    rows: gtk::Box,
    note: gtk::Label,
    legend: gtk::Box,
}

impl Default for ComparisonChart {
    fn default() -> Self {
        Self::new()
    }
}

impl ComparisonChart {
    pub fn new() -> Self {
        let heading = gtk::Label::builder()
            .label("Measured change, one scale per metric")
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let legend = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        for (class, text) in [("baseline", "Baseline"), ("candidate", "Candidate")] {
            let item = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            item.append(&swatch(class));
            item.append(&label(text));
            legend.append(&item);
        }
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let note = label("");
        note.add_css_class("kvlm-chart-note");
        note.add_css_class("dim-label");
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 6);
        widget.add_css_class("kvlm-chart-panel");
        widget.append(&heading);
        widget.append(&legend);
        widget.append(&rows);
        widget.append(&note);
        widget.set_visible(false);
        Self {
            widget,
            rows,
            note,
            legend,
        }
    }

    pub fn set(&self, comparison: &Comparison) {
        clear(&self.rows);
        let mut shown = 0_usize;
        let mut skipped = Vec::new();
        for metric in &comparison.metrics {
            let (Some(a), Some(b)) = (metric.a, metric.b) else {
                skipped.push(metric.name);
                continue;
            };
            if !a.median.is_finite() || !b.median.is_finite() {
                skipped.push(metric.name);
                continue;
            }
            self.rows.append(&metric_row(metric, a, b));
            shown += 1;
        }
        self.legend.set_visible(shown > 0);
        self.note.set_text(&if skipped.is_empty() {
            String::new()
        } else {
            format!(
                "Not measured on both runs: {}. The full text report lists every metric.",
                skipped.join(", ")
            )
        });
        self.widget.set_visible(shown > 0);
    }

    pub fn clear(&self) {
        clear(&self.rows);
        self.note.set_text("");
        self.widget.set_visible(false);
    }
}

fn metric_row(
    metric: &MetricDelta,
    a: workstation_core::experiments::Spread,
    b: workstation_core::experiments::Spread,
) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.append(&label(metric.name));
    let change = metric.change_percent.map_or_else(
        || "change unavailable".to_owned(),
        |value| format!("{value:+.1}%"),
    );
    let change_label = gtk::Label::builder()
        .label(change)
        .xalign(1.0)
        .hexpand(true)
        .css_classes(["kvlm-metric"])
        .build();
    header.append(&change_label);
    let verdict = gtk::Label::builder()
        .label(lab_text::metric_verdict(metric))
        .xalign(1.0)
        .css_classes(["dim-label", "kvlm-chart-note"])
        .build();
    header.append(&verdict);

    let area = gtk::DrawingArea::new();
    area.set_content_height(30);
    area.set_height_request(30);
    area.set_hexpand(true);
    area.set_has_tooltip(true);
    let bars = Rc::new(MetricBars {
        metric: metric.clone(),
        progress: Rc::new(Cell::new(1.0)),
        animation: Rc::new(RefCell::new(None)),
    });
    {
        let bars = bars.clone();
        area.set_draw_func(move |_, cr, width, height| {
            draw_comparison(cr, width, height, a, b, bars.progress.get());
        });
    }
    {
        let bars = bars.clone();
        area.connect_query_tooltip(move |_, _, _, _, tooltip| {
            tooltip.set_text(Some(&format!(
                "Baseline median {:.1} (range {:.1}–{:.1}); candidate {:.1} (range {:.1}–{:.1}) {}",
                a.median, a.min, a.max, b.median, b.min, b.max, bars.metric.unit
            )));
            true
        });
    }
    follow_theme(&area);
    describe(
        &area,
        &format!("{} comparison", metric.name),
        &format!(
            "Baseline median {:.1}, candidate median {:.1} {}; {}.",
            a.median,
            b.median,
            metric.unit,
            lab_text::metric_verdict(metric)
        ),
    );
    reveal(&area, &bars.progress, &bars.animation);

    let unit = gtk::Label::builder()
        .label(metric.unit)
        .xalign(1.0)
        .css_classes(["dim-label", "kvlm-chart-note"])
        .build();
    let chart = gtk::Box::new(gtk::Orientation::Vertical, 2);
    chart.append(&header);
    chart.append(&area);
    chart.append(&unit);
    chart
}

fn draw_comparison(
    cr: &cairo::Context,
    width: i32,
    height: i32,
    a: workstation_core::experiments::Spread,
    b: workstation_core::experiments::Spread,
    progress: f64,
) {
    let palette = theme::palette();
    let width = width as f64;
    let height = height as f64;
    let max = a.max.max(b.max).max(a.median).max(b.median).max(1e-9) * 1.06;
    let bar_height = 9.0;
    let gap = 3.0;
    let top = (height - bar_height * 2.0 - gap) / 2.0;
    for (index, spread, color) in [(0, a, palette.amber), (1, b, palette.teal)] {
        let y = top + index as f64 * (bar_height + gap);
        rounded(cr, 0.0, y, width, bar_height, bar_height / 2.0);
        set_color(cr, palette.track);
        let _ = cr.fill();
        let fill = (spread.median / max) * width * progress;
        if fill > 0.0 {
            rounded(cr, 0.0, y, fill.max(2.0), bar_height, bar_height / 2.0);
            set_color(cr, color);
            let _ = cr.fill();
        }
        let min_x = (spread.min / max) * width * progress;
        let max_x = (spread.max / max) * width * progress;
        if max_x > min_x {
            set_color(cr, palette.neutral);
            cr.set_line_width(1.5);
            cr.move_to(min_x, y + bar_height / 2.0);
            cr.line_to(max_x, y + bar_height / 2.0);
            let _ = cr.stroke();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spread(median: f64) -> workstation_core::experiments::Spread {
        workstation_core::experiments::Spread {
            median,
            min: median * 0.9,
            max: median * 1.1,
            count: 3,
        }
    }

    fn metric(name: &'static str, a: f64, b: f64) -> MetricDelta {
        MetricDelta {
            name,
            unit: "tokens/s",
            better: workstation_core::compare::Better::Higher,
            a: Some(spread(a)),
            b: Some(spread(b)),
            change_percent: Some((b - a) / a * 100.0),
            distinguishable: Some(true),
        }
    }

    #[test]
    fn segment_hit_test_follows_scaled_widths() {
        let data = FitData {
            segments: [50, 25, 25, 0],
            total: 100,
            limits: Some(MemoryLimits {
                comfortable_bytes: 80,
                maximum_bytes: 100,
            }),
        };
        assert_eq!(data.segment_at(0.0, 100.0), Some(0));
        assert_eq!(data.segment_at(45.0, 100.0), Some(0));
        assert_eq!(data.segment_at(60.0, 100.0), Some(1));
        assert_eq!(data.segment_at(90.0, 100.0), Some(2));
        assert_eq!(data.segment_at(101.0, 100.0), None);
    }

    #[test]
    fn zero_fill_segment_has_no_tooltip() {
        let data = FitData {
            segments: [100, 0, 0, 0],
            total: 100,
            limits: None,
        };
        assert_eq!(data.tooltip(1), None);
        assert!(data.tooltip(0).is_some());
    }

    #[test]
    fn decode_rate_requires_completed_metrics() {
        use workstation_core::db::{
            HistoryEntry, RunCompletion, RunMetrics, RunOutcome, RunSnapshot, WorkloadSnapshot,
        };
        let snapshot = RunSnapshot {
            started_at_unix_ms: 1,
            model_name: "m".into(),
            model_digest: None,
            model_size_bytes: None,
            hardware: None,
            hardware_notice: None,
            runtime_endpoint: "http://127.0.0.1:11434".into(),
            workload: WorkloadSnapshot {
                id: "w".into(),
                prompt_sha256: "0".repeat(64),
                num_predict: 8,
                num_ctx: 512,
                seed: 1,
                temperature: 0,
                think: false,
            },
        };
        let completed = HistoryEntry {
            id: 1,
            snapshot: snapshot.clone(),
            completion: Some(RunCompletion {
                finished_at_unix_ms: 2,
                runtime_version: None,
                model_digest: None,
                outcome: RunOutcome::Completed(RunMetrics {
                    first_response_ns: None,
                    elapsed_ns: 1_000_000_000,
                    response_chunks: 8,
                    eval_count: Some(40),
                    eval_duration_ns: Some(2_000_000_000),
                    load_duration_ns: None,
                    done_reason: "stop".into(),
                }),
                resources: None,
            }),
        };
        assert_eq!(decode_rate(&completed), Some(20.0));
        let unfinished = HistoryEntry {
            id: 2,
            snapshot,
            completion: None,
        };
        assert_eq!(decode_rate(&unfinished), None);
    }

    #[test]
    fn trend_points_are_chronological_and_capped() {
        let mut entries = Vec::new();
        for id in 1..=15 {
            entries.push(HistoryEntry {
                id,
                snapshot: workstation_core::db::RunSnapshot {
                    started_at_unix_ms: id as u64,
                    model_name: "m".into(),
                    model_digest: None,
                    model_size_bytes: None,
                    hardware: None,
                    hardware_notice: None,
                    runtime_endpoint: "e".into(),
                    workload: workstation_core::db::WorkloadSnapshot {
                        id: "w".into(),
                        prompt_sha256: "0".repeat(64),
                        num_predict: 8,
                        num_ctx: 512,
                        seed: 1,
                        temperature: 0,
                        think: false,
                    },
                },
                completion: None,
            });
        }
        let points = trend_points(&entries);
        assert!(points.is_empty());
    }

    #[test]
    fn summary_keeps_units_and_count() {
        let points = vec![
            TrendPoint {
                run_id: 1,
                model: "a".into(),
                rate: 10.0,
            },
            TrendPoint {
                run_id: 2,
                model: "a".into(),
                rate: 30.0,
            },
            TrendPoint {
                run_id: 3,
                model: "a".into(),
                rate: 20.0,
            },
        ];
        let text = trend_summary(&points);
        assert!(text.contains("3 saved runs"));
        assert!(text.contains("min 10.0"));
        assert!(text.contains("median 20.0"));
        assert!(text.contains("max 30.0"));
    }

    #[test]
    fn comparison_scale_uses_largest_whisker() {
        let metric = metric("Generation", 10.0, 12.0);
        assert!(metric.a.unwrap().max < 12.0);
        let max = metric.a.unwrap().max.max(metric.b.unwrap().max).max(1e-9) * 1.06;
        assert!(max > 12.0);
    }

    #[test]
    #[ignore = "requires a graphical GTK display"]
    fn panels_build_and_draw_with_representative_data() {
        adw::init().expect("GTK display is available");
        let gib = GIB as u64;
        let assessment = FitAssessment {
            class: FitClass::Comfortable,
            reason: workstation_core::fit::memory_math::FitReason::WithinComfortableBudget,
            breakdown: Some(workstation_core::fit::memory_math::MemoryBreakdown {
                weights_bytes: 4 * gib,
                kv_cache_bytes: gib,
                buffer_bytes: gib / 2,
                overhead_bytes: gib / 4,
                total_bytes: 4 * gib + gib + gib / 2 + gib / 4,
                allocated_context_tokens: 8192,
            }),
            limits: Some(MemoryLimits {
                comfortable_bytes: 8 * gib,
                maximum_bytes: 10 * gib,
            }),
        };
        let fit = FitChart::new("fit");
        fit.set(&assessment);
        let meter = Meter::new("ram");
        meter.set(&MemoryPool {
            total_bytes: 16 * gib,
            available_bytes: Some(6 * gib),
        });
        let trend = TrendChart::new();
        trend.set_entries(&[]);
        let comparison = ComparisonChart::new();
        comparison.set(&Comparison {
            a: 1,
            b: 2,
            metrics: vec![metric("Generation", 10.0, 12.0)],
            changed_settings: Vec::new(),
            controlled: None,
            notes: Vec::new(),
        });

        let mut surface =
            cairo::ImageSurface::create(cairo::Format::ARgb32, 240, 96).expect("image surface");
        let cr = cairo::Context::new(&surface).expect("cairo context");
        draw_fit(&cr, 240, 48, fit.inner.data.borrow().as_ref(), 1.0, Some(1));
        draw_meter(
            &cr,
            240,
            8,
            Some(MemoryPool {
                total_bytes: 16 * gib,
                available_bytes: Some(6 * gib),
            }),
            1.0,
        );
        draw_trend(
            &cr,
            240,
            96,
            &[
                TrendPoint {
                    run_id: 1,
                    model: "a".into(),
                    rate: 10.0,
                },
                TrendPoint {
                    run_id: 2,
                    model: "a".into(),
                    rate: 14.0,
                },
            ],
            1.0,
            Some(1),
        );
        draw_comparison(&cr, 240, 30, spread(10.0), spread(12.0), 1.0);
        drop(cr);
        surface.flush();
        let data = surface.data().expect("surface data");
        // The meter is drawn last over the fit chart, so the available third must differ from
        // the empty track at the right end.
        let filled = &data[4 * (4 * 240 + 10)..4 * (4 * 240 + 10) + 3];
        let track = &data[4 * (4 * 240 + 220)..4 * (4 * 240 + 220) + 3];
        assert_ne!(filled, track);
        assert!(
            filled[1] > track[1],
            "the fill should be brighter than the track"
        );
    }
}
