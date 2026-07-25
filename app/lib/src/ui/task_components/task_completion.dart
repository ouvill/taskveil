part of '../task_components.dart';

class AppTaskCheckbox extends StatefulWidget {
  const AppTaskCheckbox({
    super.key,
    required this.isDone,
    required this.onToggleDone,
    this.checkboxKey,
    this.tooltip,
  });

  final bool isDone;
  final VoidCallback? onToggleDone;
  final Key? checkboxKey;
  final String? tooltip;

  @override
  State<AppTaskCheckbox> createState() => _AppTaskCheckboxState();
}

class _AppTaskCheckboxState extends State<AppTaskCheckbox>
    with TickerProviderStateMixin {
  late final AnimationController _completionController;
  late final AnimationController _pressController;

  @override
  void initState() {
    super.initState();
    _completionController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 520),
      value: widget.isDone ? 1 : 0,
    );
    _pressController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 90),
    );
  }

  @override
  void didUpdateWidget(covariant AppTaskCheckbox oldWidget) {
    super.didUpdateWidget(oldWidget);
    final reduceMotion =
        MediaQuery.maybeOf(context)?.disableAnimations ?? false;
    if (!oldWidget.isDone && widget.isDone && !reduceMotion) {
      _completionController.forward(from: 0);
    } else if (oldWidget.isDone && !widget.isDone) {
      _completionController
        ..stop()
        ..value = 0;
    } else if (reduceMotion) {
      _completionController.value = widget.isDone ? 1 : 0;
    }
  }

  @override
  void dispose() {
    _completionController.dispose();
    _pressController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final colorScheme = Theme.of(context).colorScheme;
    final reduceMotion = MediaQuery.disableAnimationsOf(context);
    final mark = AnimatedBuilder(
      key: ValueKey('task-checkbox-animation-${widget.checkboxKey}'),
      animation: Listenable.merge([_completionController, _pressController]),
      builder: (context, child) {
        final timeline = widget.isDone
            ? (reduceMotion ? 1.0 : _completionController.value)
            : 0.0;
        final fillProgress = Curves.easeOutCubic.transform(
          (timeline / (200 / 520)).clamp(0.0, 1.0),
        );
        final checkProgress = Curves.easeOutCubic.transform(
          ((timeline - (130 / 520)) / (330 / 520)).clamp(0.0, 1.0),
        );
        return CustomPaint(
          size: const Size.square(_taskCheckboxVisualSize),
          painter: _TaskCheckboxPainter(
            fillProgress: fillProgress,
            checkProgress: checkProgress,
            pressProgress: _pressController.value,
            checkedColor: colorScheme.primary,
            ringColor: colorScheme.onSurfaceVariant.withValues(alpha: 0.68),
          ),
        );
      },
    );
    final control = SizedBox(
      key: widget.checkboxKey,
      width: _taskCheckboxTapSize,
      height: _taskCheckboxTapSize,
      child: widget.onToggleDone == null
          ? _TaskCheckboxVisual(
              mark: mark,
              halo: reduceMotion
                  ? null
                  : _CompletionHalo(animation: _completionController),
            )
          : InkResponse(
              onTap: _handleTap,
              radius: _taskCheckboxTapSize / 2,
              containedInkWell: true,
              customBorder: const CircleBorder(),
              child: _TaskCheckboxVisual(
                mark: mark,
                halo: reduceMotion
                    ? null
                    : _CompletionHalo(animation: _completionController),
              ),
            ),
    );
    final label = widget.tooltip;
    final semanticControl = Semantics(
      label: label,
      button: true,
      checked: widget.isDone,
      enabled: widget.onToggleDone != null,
      child: control,
    );
    if (label == null) {
      return semanticControl;
    }
    return Tooltip(message: label, child: semanticControl);
  }

  void _handleTap() {
    if (widget.onToggleDone == null) {
      return;
    }
    _pressController.forward(from: 0).then((_) {
      if (mounted) {
        _pressController.reverse();
      }
    });
    if (!widget.isDone && !MediaQuery.disableAnimationsOf(context)) {
      unawaited(HapticFeedback.lightImpact());
    }
    widget.onToggleDone!();
  }
}

class _TaskCheckboxVisual extends StatelessWidget {
  const _TaskCheckboxVisual({required this.mark, required this.halo});

  final Widget mark;
  final Widget? halo;

  @override
  Widget build(BuildContext context) {
    return Stack(
      clipBehavior: Clip.none,
      children: [
        if (halo != null) Positioned.fill(child: halo!),
        Center(child: mark),
      ],
    );
  }
}

class _TaskCheckboxPainter extends CustomPainter {
  const _TaskCheckboxPainter({
    required this.fillProgress,
    required this.checkProgress,
    required this.pressProgress,
    required this.checkedColor,
    required this.ringColor,
  });

  final double fillProgress;
  final double checkProgress;
  final double pressProgress;
  final Color checkedColor;
  final Color ringColor;

  static const double _ringStrokeWidth = 1;
  static const double _checkStrokeWidth = 1.4;

  @override
  void paint(Canvas canvas, Size size) {
    final clampedFill = fillProgress.clamp(0.0, 1.0);
    final clampedCheck = checkProgress.clamp(0.0, 1.0);
    final center = size.center(Offset.zero);
    final radius = (math.min(size.width, size.height) - _ringStrokeWidth) / 2;
    final ringPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = _ringStrokeWidth + (pressProgress * 0.35)
      ..strokeCap = StrokeCap.round
      ..color = ringColor.withValues(alpha: 1 - (clampedFill * 0.42));
    canvas.drawCircle(center, radius, ringPaint);

    if (clampedFill <= 0) {
      return;
    }

    final fillScale = (0.82 + (clampedFill * 0.18)).clamp(0.0, 1.0);
    final fillPaint = Paint()
      ..style = PaintingStyle.fill
      ..color = checkedColor.withValues(alpha: clampedFill);
    canvas.save();
    canvas.translate(center.dx, center.dy);
    canvas.scale(fillScale);
    canvas.drawCircle(Offset.zero, radius, fillPaint);
    canvas.restore();

    if (clampedCheck <= 0) {
      return;
    }
    final checkPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = _checkStrokeWidth
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round
      ..color = Colors.white.withValues(alpha: clampedCheck);
    final path = Path()
      ..moveTo(size.width * 0.29, size.height * 0.52)
      ..lineTo(size.width * 0.44, size.height * 0.67)
      ..lineTo(size.width * 0.73, size.height * 0.35);
    final metric = path.computeMetrics().single;
    canvas.drawPath(
      metric.extractPath(0, metric.length * clampedCheck),
      checkPaint,
    );
  }

  @override
  bool shouldRepaint(covariant _TaskCheckboxPainter oldDelegate) {
    return oldDelegate.fillProgress != fillProgress ||
        oldDelegate.checkProgress != checkProgress ||
        oldDelegate.pressProgress != pressProgress ||
        oldDelegate.checkedColor != checkedColor ||
        oldDelegate.ringColor != ringColor;
  }
}

class _CompletionHalo extends StatelessWidget {
  const _CompletionHalo({required this.animation});

  final Animation<double> animation;

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: animation,
      builder: (context, child) {
        if (animation.value == 0 || animation.value == 1) {
          return const SizedBox.shrink();
        }
        return CustomPaint(
          key: _taskCompletionHaloKey,
          painter: _CompletionHaloPainter(
            progress: animation.value,
            color: Theme.of(context).colorScheme.primary,
          ),
        );
      },
    );
  }
}

class _CompletionHaloPainter extends CustomPainter {
  const _CompletionHaloPainter({required this.progress, required this.color});

  final double progress;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final raw = ((progress - 0.22) / 0.78).clamp(0.0, 1.0);
    if (raw <= 0) {
      return;
    }
    final travel = Curves.easeOutCubic.transform(raw);
    final opacity = (1 - Curves.easeInCubic.transform(raw)) * 0.42;
    final origin = size.center(Offset.zero);
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.2
      ..color = color.withValues(alpha: opacity);
    canvas.drawCircle(origin, 11 + (10 * travel), paint);
  }

  @override
  bool shouldRepaint(covariant _CompletionHaloPainter oldDelegate) {
    return oldDelegate.progress != progress || oldDelegate.color != color;
  }
}

class AppAnimatedTaskTitle extends StatefulWidget {
  const AppAnimatedTaskTitle(
    this.data, {
    super.key,
    required this.isDone,
    this.style,
    this.strutStyle,
    this.maxLines,
    this.overflow,
    this.softWrap,
    this.textKey,
  });

  final String data;
  final bool isDone;
  final TextStyle? style;
  final StrutStyle? strutStyle;
  final int? maxLines;
  final TextOverflow? overflow;
  final bool? softWrap;
  final Key? textKey;

  @override
  State<AppAnimatedTaskTitle> createState() => _AppAnimatedTaskTitleState();
}

class _AppAnimatedTaskTitleState extends State<AppAnimatedTaskTitle>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;
  bool _drawingAnimatedStrike = false;

  @override
  void initState() {
    super.initState();
    _controller =
        AnimationController(
          vsync: this,
          duration: const Duration(milliseconds: 460),
        )..addStatusListener((status) {
          if (status == AnimationStatus.completed && mounted) {
            setState(() => _drawingAnimatedStrike = false);
          }
        });
  }

  @override
  void didUpdateWidget(covariant AppAnimatedTaskTitle oldWidget) {
    super.didUpdateWidget(oldWidget);
    final reduceMotion =
        MediaQuery.maybeOf(context)?.disableAnimations ?? false;
    if (!oldWidget.isDone && widget.isDone && !reduceMotion) {
      setState(() => _drawingAnimatedStrike = true);
      _controller.forward(from: 0);
    } else if (oldWidget.isDone && !widget.isDone) {
      _controller.stop();
      _controller.value = 0;
      _drawingAnimatedStrike = false;
    } else if (reduceMotion && _drawingAnimatedStrike) {
      _controller.stop();
      _controller.value = 1;
      _drawingAnimatedStrike = false;
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final reduceMotion = MediaQuery.disableAnimationsOf(context);
    final drawAnimatedStrike =
        widget.isDone && _drawingAnimatedStrike && !reduceMotion;
    final drawStrike = widget.isDone;
    final effectiveStyle = drawStrike
        ? widget.style?.copyWith(decoration: TextDecoration.none)
        : widget.style;
    final text = Text(
      widget.data,
      key: widget.textKey,
      maxLines: widget.maxLines,
      overflow: widget.overflow,
      softWrap: widget.softWrap,
      strutStyle: widget.strutStyle,
      style: effectiveStyle,
    );
    if (!drawStrike) {
      return text;
    }

    return Stack(
      fit: StackFit.passthrough,
      children: [
        text,
        Positioned.fill(
          key: _taskStrikethroughOverlayKey,
          child: IgnorePointer(
            child: AnimatedBuilder(
              animation: _controller,
              builder: (context, child) {
                final strikeProgress = drawAnimatedStrike
                    ? Curves.easeOutCubic.transform(
                        ((_controller.value - (130 / 460)) / (330 / 460)).clamp(
                          0.0,
                          1.0,
                        ),
                      )
                    : 1.0;
                return CustomPaint(
                  painter: _AnimatedStrikethroughPainter(
                    text: widget.data,
                    style: widget.style,
                    strutStyle: widget.strutStyle,
                    maxLines: widget.maxLines,
                    overflow: widget.overflow,
                    textDirection: Directionality.of(context),
                    locale: Localizations.maybeLocaleOf(context),
                    textScaler: MediaQuery.textScalerOf(context),
                    progress: strikeProgress,
                  ),
                );
              },
            ),
          ),
        ),
      ],
    );
  }
}

class _AnimatedStrikethroughPainter extends CustomPainter {
  const _AnimatedStrikethroughPainter({
    required this.text,
    required this.style,
    required this.strutStyle,
    required this.maxLines,
    required this.overflow,
    required this.textDirection,
    required this.locale,
    required this.textScaler,
    required this.progress,
  });

  final String text;
  final TextStyle? style;
  final StrutStyle? strutStyle;
  final int? maxLines;
  final TextOverflow? overflow;
  final TextDirection textDirection;
  final Locale? locale;
  final TextScaler textScaler;
  final double progress;

  @override
  void paint(Canvas canvas, Size size) {
    final textStyle = (style ?? const TextStyle()).copyWith(
      decoration: TextDecoration.none,
    );
    final painter = TextPainter(
      text: TextSpan(text: text, style: textStyle),
      textDirection: textDirection,
      maxLines: maxLines,
      ellipsis: overflow == TextOverflow.ellipsis ? '\u2026' : null,
      locale: locale,
      strutStyle: strutStyle,
      textScaler: textScaler,
    )..layout(maxWidth: size.width);
    final lines = painter.computeLineMetrics();
    if (lines.isEmpty) {
      return;
    }

    final strikeColor =
        style?.decorationColor ?? style?.color ?? const Color(0xFF000000);
    final fontSize = textStyle.fontSize ?? 14;
    final strokeWidth = math.max(
      1.0,
      (style?.decorationThickness ?? 1.0) * (fontSize / 14),
    );
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeCap = StrokeCap.round
      ..strokeWidth = strokeWidth
      ..color = strikeColor;
    final scaledProgress = (progress.clamp(0.0, 1.0)) * lines.length;
    for (var i = 0; i < lines.length; i += 1) {
      final lineProgress = (scaledProgress - i).clamp(0.0, 1.0);
      if (lineProgress <= 0) {
        continue;
      }
      final line = lines[i];
      final startX = line.left;
      final endX = startX + (line.width * lineProgress);
      final y = line.baseline - (line.ascent * 0.34);
      canvas.drawLine(Offset(startX, y), Offset(endX, y), paint);
    }
  }

  @override
  bool shouldRepaint(covariant _AnimatedStrikethroughPainter oldDelegate) {
    return oldDelegate.text != text ||
        oldDelegate.style != style ||
        oldDelegate.strutStyle != strutStyle ||
        oldDelegate.maxLines != maxLines ||
        oldDelegate.overflow != overflow ||
        oldDelegate.textDirection != textDirection ||
        oldDelegate.locale != locale ||
        oldDelegate.textScaler != textScaler ||
        oldDelegate.progress != progress;
  }
}

/// A small priority indicator dot shown in a task metadata row. Uses the
/// design-direction accent tokens
/// (coral/amber/softSage) and always carries a [semanticLabel] + tooltip so
/// priority meaning does not rely on color alone. Renders nothing for
/// priority "none" (0), per the design direction's dot-only convention.
