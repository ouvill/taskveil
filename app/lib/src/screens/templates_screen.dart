import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';
import 'package:intl/intl.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/ui/states.dart';
import 'package:taskveil/src/ui/bridge_error_messages.dart';
import 'package:taskveil/src/ui/task_components.dart';
import 'package:taskveil/src/ui/theme.dart';

const _emptyBlueprintText = '';

class TemplatesScreen extends ConsumerStatefulWidget {
  const TemplatesScreen({super.key});

  @override
  ConsumerState<TemplatesScreen> createState() => _TemplatesScreenState();
}

class _TemplatesScreenState extends ConsumerState<TemplatesScreen> {
  bool _loading = true;
  Object? _error;
  List<TemplateDto> _templates = const [];
  List<ListDto> _lists = const [];
  List<TaskSeriesDto> _series = const [];
  final Map<String, StreakDto> _streaks = {};

  @override
  void initState() {
    super.initState();
    _reload();
  }

  Future<void> _reload() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final bridge = ref.read(bridgeServiceProvider);
      final templates = await bridge.getTemplates();
      final lists = await bridge.getLists();
      final series = await bridge.getTaskSeries();
      final streaks = <String, StreakDto>{};
      for (final value in series) {
        streaks[value.id] = await bridge.getTaskSeriesStreak(
          seriesId: value.id,
          atMs: DateTime.now().millisecondsSinceEpoch,
        );
      }
      if (!mounted) return;
      setState(() {
        _templates = templates;
        _lists = lists;
        _series = series;
        _streaks
          ..clear()
          ..addAll(streaks);
        _loading = false;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _error = error;
        _loading = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    return Scaffold(
      backgroundColor: AppColors.canvas,
      appBar: AppBar(
        leading: IconButton(
          tooltip: l10n.backButtonTooltip,
          onPressed: () => context.pop(),
          icon: const Icon(LucideIcons.arrowLeft300),
        ),
        title: Text(l10n.templatesTitle),
      ),
      body: SafeArea(
        child: _loading
            ? const AppLoadingState()
            : _error != null
            ? AppErrorState(
                message: l10n.templatesLoadFailed(
                  bridgeErrorMessage(l10n, _error!),
                ),
              )
            : RefreshIndicator(
                onRefresh: _reload,
                child: ListView(
                  padding: const EdgeInsets.fromLTRB(
                    AppSpacing.md,
                    AppSpacing.sm,
                    AppSpacing.md,
                    AppSpacing.xl,
                  ),
                  children: [
                    Row(
                      children: [
                        Expanded(
                          child: Text(
                            l10n.templatesTitle,
                            style: Theme.of(context).textTheme.titleLarge,
                          ),
                        ),
                        IconButton.filledTonal(
                          key: const Key('create-template'),
                          tooltip: l10n.newTemplateButton,
                          onPressed: _createTemplate,
                          icon: const Icon(LucideIcons.plus300),
                        ),
                      ],
                    ),
                    const SizedBox(height: AppSpacing.md),
                    if (_templates.isEmpty)
                      AppEmptyState(
                        icon: LucideIcons.copyPlus300,
                        title: l10n.templatesEmptyTitle,
                        body: l10n.templatesEmptyBody,
                      )
                    else
                      for (final template in _templates)
                        _TemplateCard(
                          template: template,
                          lists: _lists,
                          onChanged: _reload,
                        ),
                    const SizedBox(height: AppSpacing.lg),
                    Text(
                      l10n.taskSeriesTitle,
                      style: Theme.of(context).textTheme.titleLarge,
                    ),
                    const SizedBox(height: AppSpacing.sm),
                    if (_series.isEmpty)
                      AppEmptyState(
                        icon: LucideIcons.repeat2300,
                        title: l10n.taskSeriesEmptyTitle,
                        body: l10n.taskSeriesEmptyBody,
                      )
                    else
                      for (final series in _series)
                        _TaskSeriesCard(
                          series: series,
                          lists: _lists,
                          streak: _streaks[series.id],
                          onChanged: _reload,
                        ),
                  ],
                ),
              ),
      ),
    );
  }

  Future<void> _createTemplate() async {
    final value = await _showTemplateEditDialog(context, null, _lists);
    if (value == null || !mounted) return;
    await ref
        .read(bridgeServiceProvider)
        .createTemplate(
          name: value.name,
          defaultListId: value.defaultListId,
          nodes: value.nodes,
        );
    await _reload();
  }
}

class _TemplateCard extends ConsumerWidget {
  const _TemplateCard({
    required this.template,
    required this.lists,
    required this.onChanged,
  });

  final TemplateDto template;
  final List<ListDto> lists;
  final Future<void> Function() onChanged;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = AppLocalizations.of(context)!;
    final colors = Theme.of(context).colorScheme;
    return Card(
      margin: const EdgeInsets.only(bottom: AppSpacing.md),
      child: Padding(
        padding: const EdgeInsets.all(AppSpacing.md),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    template.name,
                    style: Theme.of(context).textTheme.titleMedium,
                  ),
                ),
                PopupMenuButton<String>(
                  tooltip: l10n.templateActionsTooltip,
                  onSelected: (value) =>
                      _handleTemplateAction(context, ref, value),
                  itemBuilder: (context) => [
                    PopupMenuItem(value: 'edit', child: Text(l10n.editButton)),
                    PopupMenuItem(
                      value: 'duplicate',
                      child: Text(l10n.duplicateTemplateMenuItem),
                    ),
                    PopupMenuItem(
                      value: 'replace',
                      child: Text(l10n.replaceTemplateSnapshotMenuItem),
                    ),
                    const PopupMenuDivider(),
                    PopupMenuItem(
                      value: 'delete',
                      child: Text(l10n.deleteButton),
                    ),
                  ],
                ),
              ],
            ),
            Text(
              l10n.templateTaskCount(template.nodes.length),
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: colors.onSurfaceVariant),
            ),
            const SizedBox(height: AppSpacing.sm),
            Row(
              children: [
                Expanded(
                  child: FilledButton.tonalIcon(
                    onPressed: () => _instantiate(context, ref),
                    icon: const Icon(LucideIcons.play300),
                    label: Text(l10n.createFromTemplateButton),
                  ),
                ),
                const SizedBox(width: AppSpacing.sm),
                IconButton.filledTonal(
                  tooltip: l10n.createTaskSeriesTooltip,
                  onPressed: () => _createSeries(context, ref),
                  icon: const Icon(LucideIcons.calendarPlus300),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _instantiate(BuildContext context, WidgetRef ref) async {
    final l10n = AppLocalizations.of(context)!;
    await ref
        .read(bridgeServiceProvider)
        .instantiateTemplate(templateId: template.id);
    ref.invalidate(listsProvider);
    if (context.mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(l10n.templateCreatedMessage)));
    }
  }

  Future<void> _handleTemplateAction(
    BuildContext context,
    WidgetRef ref,
    String action,
  ) async {
    final l10n = AppLocalizations.of(context)!;
    if (action == 'edit') {
      final value = await _showTemplateEditDialog(context, template, lists);
      if (value == null) return;
      await ref
          .read(bridgeServiceProvider)
          .updateTemplate(
            templateId: template.id,
            name: value.name,
            defaultListId: value.defaultListId,
            nodes: value.nodes,
          );
    } else if (action == 'duplicate') {
      await ref
          .read(bridgeServiceProvider)
          .createTemplate(
            name: l10n.templateCopyName(template.name),
            defaultListId: template.defaultListId,
            nodes: template.nodes,
          );
    } else if (action == 'replace') {
      final taskId = await _showTaskIdDialog(context);
      if (taskId == null) return;
      await ref
          .read(bridgeServiceProvider)
          .replaceTemplateBlueprint(templateId: template.id, taskId: taskId);
    } else if (action == 'delete') {
      final confirmed = await _confirmTemplateDelete(context, template.name);
      if (!confirmed) return;
      await ref
          .read(bridgeServiceProvider)
          .deleteTemplate(templateId: template.id);
    }
    await onChanged();
  }

  Future<void> _createSeries(BuildContext context, WidgetRef ref) async {
    final bridge = ref.read(bridgeServiceProvider);
    final defaultTimeZone = await bridge.getLocalTimeZone();
    if (!context.mounted) return;
    final input = await showTaskSeriesDialog(context, null, defaultTimeZone);
    if (input == null) return;
    try {
      await bridge.validateRecurrenceRule(
        rrule: input.rrule,
        startsAt: input.startsAt,
        timeZone: input.timeZone,
      );
    } catch (_) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text(
              AppLocalizations.of(context)!.scheduleValidationFailed,
            ),
          ),
        );
      }
      return;
    }
    await bridge.createTaskSeriesFromTemplate(
      templateId: template.id,
      rrule: input.rrule,
      startsAt: input.startsAt,
      timeZone: input.timeZone,
    );
    await onChanged();
  }
}

class _TaskSeriesCard extends ConsumerWidget {
  const _TaskSeriesCard({
    required this.series,
    required this.lists,
    required this.streak,
    required this.onChanged,
  });

  final TaskSeriesDto series;
  final List<ListDto> lists;
  final StreakDto? streak;
  final Future<void> Function() onChanged;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return Card(
      margin: const EdgeInsets.only(bottom: AppSpacing.md),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: AppSpacing.md),
        child: _TaskSeriesRow(
          series: series,
          streak: streak,
          onEdit: () => _edit(context, ref),
          onEditContent: () => _editContent(context, ref),
          onToggle: () => _toggle(ref),
          onDelete: () => _delete(context, ref),
        ),
      ),
    );
  }

  Future<void> _edit(BuildContext context, WidgetRef ref) async {
    final bridge = ref.read(bridgeServiceProvider);
    final input = await showTaskSeriesDialog(context, series, series.timeZone);
    if (input == null) return;
    try {
      await bridge.validateRecurrenceRule(
        rrule: input.rrule,
        startsAt: input.startsAt,
        timeZone: input.timeZone,
      );
    } catch (_) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text(
              AppLocalizations.of(context)!.scheduleValidationFailed,
            ),
          ),
        );
      }
      return;
    }
    await bridge.updateTaskSeries(
      seriesId: series.id,
      targetListId: series.targetListId,
      nodes: series.nodes,
      rrule: input.rrule,
      startsAt: input.startsAt,
      timeZone: input.timeZone,
      enabled: input.enabled,
    );
    await onChanged();
  }

  Future<void> _toggle(WidgetRef ref) async {
    await ref
        .read(bridgeServiceProvider)
        .updateTaskSeries(
          seriesId: series.id,
          targetListId: series.targetListId,
          nodes: series.nodes,
          rrule: series.rrule,
          startsAt: series.startsAt,
          timeZone: series.timeZone,
          enabled: !series.enabled,
        );
    await onChanged();
  }

  Future<void> _editContent(BuildContext context, WidgetRef ref) async {
    final value = await _showSeriesContentDialog(context, series, lists);
    if (value == null) return;
    await ref
        .read(bridgeServiceProvider)
        .updateTaskSeries(
          seriesId: series.id,
          targetListId: value.targetListId,
          nodes: value.nodes,
          rrule: series.rrule,
          startsAt: series.startsAt,
          timeZone: series.timeZone,
          enabled: series.enabled,
        );
    await onChanged();
  }

  Future<void> _delete(BuildContext context, WidgetRef ref) async {
    final l10n = AppLocalizations.of(context)!;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(l10n.deleteScheduleDialogTitle),
        content: Text(l10n.deleteScheduleDialogBody),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: Text(l10n.cancelButton),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: Text(l10n.deleteButton),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    await ref.read(bridgeServiceProvider).deleteTaskSeries(seriesId: series.id);
    await onChanged();
  }
}

class _TaskSeriesRow extends StatelessWidget {
  const _TaskSeriesRow({
    required this.series,
    required this.streak,
    required this.onEdit,
    required this.onEditContent,
    required this.onToggle,
    required this.onDelete,
  });

  final TaskSeriesDto series;
  final StreakDto? streak;
  final VoidCallback onEdit;
  final VoidCallback onEditContent;
  final VoidCallback onToggle;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final locale = Localizations.localeOf(context).toLanguageTag();
    final next = series.nextRunAt == null
        ? l10n.scheduleEndedLabel
        : DateFormat.yMMMd(locale).add_jm().format(
            DateTime.fromMillisecondsSinceEpoch(series.nextRunAt!),
          );
    return Semantics(
      label: l10n.scheduleSemantics(series.rrule, next),
      child: ListTile(
        contentPadding: EdgeInsets.zero,
        leading: Icon(
          series.enabled ? LucideIcons.repeat2300 : LucideIcons.pause300,
        ),
        title: Text(series.rrule),
        subtitle: Text(
          streak == null || streak!.current == 0
              ? next
              : '$next · ${l10n.scheduleStreak(streak!.current)}',
        ),
        trailing: PopupMenuButton<String>(
          tooltip: l10n.scheduleActionsTooltip,
          onSelected: (value) {
            if (value == 'edit') onEdit();
            if (value == 'edit-content') onEditContent();
            if (value == 'toggle') onToggle();
            if (value == 'delete') onDelete();
          },
          itemBuilder: (context) => [
            PopupMenuItem(value: 'edit', child: Text(l10n.editButton)),
            PopupMenuItem(
              value: 'edit-content',
              child: Text(l10n.editSeriesContentMenuItem),
            ),
            PopupMenuItem(
              value: 'toggle',
              child: Text(
                series.enabled
                    ? l10n.pauseScheduleMenuItem
                    : l10n.resumeScheduleMenuItem,
              ),
            ),
            PopupMenuItem(value: 'delete', child: Text(l10n.deleteButton)),
          ],
        ),
      ),
    );
  }
}

typedef _TemplateEditValue = ({
  String name,
  String? defaultListId,
  List<TaskBlueprintNodeDto> nodes,
});

typedef _SeriesContentValue = ({
  String? targetListId,
  List<TaskBlueprintNodeDto> nodes,
});

Future<_SeriesContentValue?> _showSeriesContentDialog(
  BuildContext context,
  TaskSeriesDto series,
  List<ListDto> lists,
) {
  final l10n = AppLocalizations.of(context)!;
  var targetListId = series.targetListId;
  var nodes = series.nodes.toList();
  return showDialog<_SeriesContentValue>(
    context: context,
    builder: (context) => StatefulBuilder(
      builder: (context, setState) => AlertDialog(
        title: Text(l10n.editSeriesContentTitle),
        content: SizedBox(
          width: 520,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                DropdownButtonFormField<String?>(
                  initialValue: targetListId,
                  isExpanded: true,
                  decoration: InputDecoration(labelText: l10n.targetListLabel),
                  items: [
                    DropdownMenuItem(
                      value: null,
                      child: Text(l10n.inboxFallbackLabel),
                    ),
                    for (final list in lists)
                      DropdownMenuItem(value: list.id, child: Text(list.name)),
                  ],
                  onChanged: (value) => setState(() => targetListId = value),
                ),
                const SizedBox(height: AppSpacing.lg),
                for (var index = 0; index < nodes.length; index++)
                  _BlueprintNodeEditor(
                    key: ValueKey(nodes[index].nodeKey),
                    node: nodes[index],
                    isRoot: nodes[index].parentNodeKey == null,
                    canMoveUp: _canMoveBlueprintNode(nodes, index, -1),
                    canMoveDown: _canMoveBlueprintNode(nodes, index, 1),
                    onTitleChanged: (value) => setState(() {
                      nodes[index] = _copyBlueprintNode(
                        nodes[index],
                        title: value,
                      );
                    }),
                    onNoteChanged: (value) => setState(() {
                      nodes[index] = _copyBlueprintNode(
                        nodes[index],
                        note: value,
                      );
                    }),
                    onPriorityChanged: (value) => setState(() {
                      nodes[index] = _copyBlueprintNode(
                        nodes[index],
                        priority: value,
                      );
                    }),
                    onEstimatedMinutesChanged: (value) => setState(() {
                      nodes[index] = _copyBlueprintNode(
                        nodes[index],
                        estimatedMinutes: value,
                        replaceEstimatedMinutes: true,
                      );
                    }),
                    onMoveUp: () => setState(() {
                      nodes = _moveBlueprintNode(nodes, index, -1);
                    }),
                    onMoveDown: () => setState(() {
                      nodes = _moveBlueprintNode(nodes, index, 1);
                    }),
                    onDelete: () => setState(() {
                      nodes = _removeBlueprintSubtree(
                        nodes,
                        nodes[index].nodeKey,
                      );
                    }),
                  ),
                Align(
                  alignment: Alignment.centerLeft,
                  child: TextButton.icon(
                    key: const Key('add-series-blueprint-child'),
                    onPressed: () => setState(() {
                      nodes = _appendBlueprintChild(nodes);
                    }),
                    icon: const Icon(LucideIcons.plus300),
                    label: Text(l10n.addBlueprintChildButton),
                  ),
                ),
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: Text(l10n.cancelButton),
          ),
          FilledButton(
            key: const Key('save-series-content'),
            onPressed: nodes.any((node) => node.title.trim().isEmpty)
                ? null
                : () => Navigator.pop(context, (
                    targetListId: targetListId,
                    nodes: _normalizeBlueprintOrder(nodes),
                  )),
            child: Text(l10n.saveButton),
          ),
        ],
      ),
    ),
  );
}

Future<_TemplateEditValue?> _showTemplateEditDialog(
  BuildContext context,
  TemplateDto? template,
  List<ListDto> lists,
) {
  final l10n = AppLocalizations.of(context)!;
  final controller = TextEditingController(text: template?.name ?? '');
  var listId = template?.defaultListId;
  var nodes =
      template?.nodes.toList() ??
      [
        const TaskBlueprintNodeDto(
          nodeKey: 'root',
          siblingOrder: 0,
          title: _emptyBlueprintText,
          note: '',
          priority: 0,
        ),
      ];
  return showDialog<_TemplateEditValue>(
    context: context,
    builder: (context) => StatefulBuilder(
      builder: (context, setState) => AlertDialog(
        title: Text(
          template == null ? l10n.newTemplateTitle : l10n.editTemplateTitle,
        ),
        content: SizedBox(
          width: 520,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                  key: const Key('template-name'),
                  controller: controller,
                  decoration: InputDecoration(labelText: l10n.nameLabel),
                  onChanged: (_) => setState(() {}),
                ),
                const SizedBox(height: AppSpacing.md),
                DropdownButtonFormField<String?>(
                  initialValue: listId,
                  isExpanded: true,
                  decoration: InputDecoration(labelText: l10n.defaultListLabel),
                  items: [
                    DropdownMenuItem(
                      value: null,
                      child: Text(l10n.inboxFallbackLabel),
                    ),
                    for (final list in lists)
                      DropdownMenuItem(value: list.id, child: Text(list.name)),
                  ],
                  onChanged: (value) => setState(() => listId = value),
                ),
                const SizedBox(height: AppSpacing.lg),
                for (var index = 0; index < nodes.length; index++)
                  _BlueprintNodeEditor(
                    key: ValueKey(nodes[index].nodeKey),
                    node: nodes[index],
                    isRoot: nodes[index].parentNodeKey == null,
                    canMoveUp: _canMoveBlueprintNode(nodes, index, -1),
                    canMoveDown: _canMoveBlueprintNode(nodes, index, 1),
                    onTitleChanged: (value) => setState(() {
                      nodes[index] = _copyBlueprintNode(
                        nodes[index],
                        title: value,
                      );
                    }),
                    onNoteChanged: (value) => setState(() {
                      nodes[index] = _copyBlueprintNode(
                        nodes[index],
                        note: value,
                      );
                    }),
                    onPriorityChanged: (value) => setState(() {
                      nodes[index] = _copyBlueprintNode(
                        nodes[index],
                        priority: value,
                      );
                    }),
                    onEstimatedMinutesChanged: (value) => setState(() {
                      nodes[index] = _copyBlueprintNode(
                        nodes[index],
                        estimatedMinutes: value,
                        replaceEstimatedMinutes: true,
                      );
                    }),
                    onMoveUp: () => setState(() {
                      nodes = _moveBlueprintNode(nodes, index, -1);
                    }),
                    onMoveDown: () => setState(() {
                      nodes = _moveBlueprintNode(nodes, index, 1);
                    }),
                    onDelete: () => setState(() {
                      nodes = _removeBlueprintSubtree(
                        nodes,
                        nodes[index].nodeKey,
                      );
                    }),
                  ),
                Align(
                  alignment: Alignment.centerLeft,
                  child: TextButton.icon(
                    key: const Key('add-blueprint-child'),
                    onPressed: () => setState(() {
                      nodes = _appendBlueprintChild(nodes);
                    }),
                    icon: const Icon(LucideIcons.plus300),
                    label: Text(l10n.addBlueprintChildButton),
                  ),
                ),
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: Text(l10n.cancelButton),
          ),
          FilledButton(
            key: const Key('save-template'),
            onPressed:
                controller.text.trim().isEmpty ||
                    nodes.any((node) => node.title.trim().isEmpty)
                ? null
                : () => Navigator.pop(context, (
                    name: controller.text.trim(),
                    defaultListId: listId,
                    nodes: _normalizeBlueprintOrder(nodes),
                  )),
            child: Text(l10n.saveButton),
          ),
        ],
      ),
    ),
  );
}

class _BlueprintNodeEditor extends StatelessWidget {
  const _BlueprintNodeEditor({
    super.key,
    required this.node,
    required this.isRoot,
    required this.canMoveUp,
    required this.canMoveDown,
    required this.onTitleChanged,
    required this.onNoteChanged,
    required this.onPriorityChanged,
    required this.onEstimatedMinutesChanged,
    required this.onMoveUp,
    required this.onMoveDown,
    required this.onDelete,
  });

  final TaskBlueprintNodeDto node;
  final bool isRoot;
  final bool canMoveUp;
  final bool canMoveDown;
  final ValueChanged<String> onTitleChanged;
  final ValueChanged<String> onNoteChanged;
  final ValueChanged<int> onPriorityChanged;
  final ValueChanged<int?> onEstimatedMinutesChanged;
  final VoidCallback onMoveUp;
  final VoidCallback onMoveDown;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    return Card.outlined(
      margin: const EdgeInsets.only(bottom: AppSpacing.sm),
      child: Padding(
        padding: const EdgeInsets.all(AppSpacing.sm),
        child: Column(
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    isRoot ? l10n.blueprintRootLabel : l10n.blueprintChildLabel,
                    style: Theme.of(context).textTheme.labelLarge,
                  ),
                ),
                if (!isRoot) ...[
                  IconButton(
                    tooltip: l10n.moveUpButton,
                    onPressed: canMoveUp ? onMoveUp : null,
                    icon: const Icon(LucideIcons.arrowUp300),
                  ),
                  IconButton(
                    tooltip: l10n.moveDownButton,
                    onPressed: canMoveDown ? onMoveDown : null,
                    icon: const Icon(LucideIcons.arrowDown300),
                  ),
                  IconButton(
                    tooltip: l10n.deleteButton,
                    onPressed: onDelete,
                    icon: const Icon(LucideIcons.trash2300),
                  ),
                ],
              ],
            ),
            TextFormField(
              key: ValueKey('blueprint-title-${node.nodeKey}'),
              initialValue: node.title,
              decoration: InputDecoration(labelText: l10n.titleLabel),
              onChanged: onTitleChanged,
            ),
            const SizedBox(height: AppSpacing.sm),
            TextFormField(
              key: ValueKey('blueprint-note-${node.nodeKey}'),
              initialValue: node.note,
              decoration: InputDecoration(labelText: l10n.noteLabel),
              minLines: 1,
              maxLines: 3,
              onChanged: onNoteChanged,
            ),
            const SizedBox(height: AppSpacing.sm),
            DropdownButtonFormField<int>(
              key: ValueKey('blueprint-priority-${node.nodeKey}'),
              initialValue: node.priority,
              decoration: InputDecoration(labelText: l10n.priorityLabel),
              items: [
                for (var priority = 0; priority <= 3; priority++)
                  DropdownMenuItem(
                    value: priority,
                    child: Text(taskPriorityLabel(l10n, priority)),
                  ),
              ],
              onChanged: (value) {
                if (value != null) {
                  onPriorityChanged(value);
                }
              },
            ),
            const SizedBox(height: AppSpacing.sm),
            DropdownButtonFormField<int?>(
              key: ValueKey('blueprint-estimate-${node.nodeKey}'),
              initialValue: node.estimatedMinutes,
              decoration: InputDecoration(labelText: l10n.estimateLabel),
              items: [
                DropdownMenuItem(value: null, child: Text(l10n.estimateNotSet)),
                for (final minutes in _estimateChoices(node.estimatedMinutes))
                  DropdownMenuItem(
                    value: minutes,
                    child: Text(l10n.estimateMinutes(minutes)),
                  ),
              ],
              onChanged: onEstimatedMinutesChanged,
            ),
          ],
        ),
      ),
    );
  }
}

TaskBlueprintNodeDto _copyBlueprintNode(
  TaskBlueprintNodeDto node, {
  String? title,
  String? note,
  int? siblingOrder,
  int? priority,
  int? estimatedMinutes,
  bool replaceEstimatedMinutes = false,
}) => TaskBlueprintNodeDto(
  nodeKey: node.nodeKey,
  parentNodeKey: node.parentNodeKey,
  siblingOrder: siblingOrder ?? node.siblingOrder,
  title: title ?? node.title,
  note: note ?? node.note,
  priority: priority ?? node.priority,
  estimatedMinutes: replaceEstimatedMinutes
      ? estimatedMinutes
      : node.estimatedMinutes,
);

List<TaskBlueprintNodeDto> _normalizeBlueprintOrder(
  List<TaskBlueprintNodeDto> nodes,
) {
  final siblingIndexes = <String?, int>{};
  return [
    for (final node in nodes)
      _copyBlueprintNode(
        node,
        siblingOrder: siblingIndexes.update(
          node.parentNodeKey,
          (value) => value + 1,
          ifAbsent: () => 0,
        ),
      ),
  ];
}

List<int> _estimateChoices(int? current) {
  final values = <int>{5, 10, 15, 30, 45, 60, 90, 120};
  if (current != null) values.add(current);
  return values.toList()..sort();
}

List<TaskBlueprintNodeDto> _appendBlueprintChild(
  List<TaskBlueprintNodeDto> nodes,
) {
  final rootKey = nodes
      .firstWhere((node) => node.parentNodeKey == null)
      .nodeKey;
  return _normalizeBlueprintOrder([
    ...nodes,
    TaskBlueprintNodeDto(
      nodeKey: 'node-${DateTime.now().microsecondsSinceEpoch}-${nodes.length}',
      parentNodeKey: rootKey,
      siblingOrder: 0,
      title: _emptyBlueprintText,
      note: '',
      priority: 0,
    ),
  ]);
}

bool _canMoveBlueprintNode(
  List<TaskBlueprintNodeDto> nodes,
  int index,
  int direction,
) {
  final node = nodes[index];
  if (node.parentNodeKey == null) return false;
  final siblings = [
    for (var candidate = 0; candidate < nodes.length; candidate++)
      if (nodes[candidate].parentNodeKey == node.parentNodeKey) candidate,
  ];
  final siblingIndex = siblings.indexOf(index);
  final target = siblingIndex + direction;
  return target >= 0 && target < siblings.length;
}

List<TaskBlueprintNodeDto> _moveBlueprintNode(
  List<TaskBlueprintNodeDto> nodes,
  int index,
  int direction,
) {
  if (!_canMoveBlueprintNode(nodes, index, direction)) return nodes;
  final node = nodes[index];
  final siblings = [
    for (var candidate = 0; candidate < nodes.length; candidate++)
      if (nodes[candidate].parentNodeKey == node.parentNodeKey) candidate,
  ];
  final targetIndex = siblings[siblings.indexOf(index) + direction];
  final moved = nodes.toList();
  final target = moved[targetIndex];
  moved[targetIndex] = moved[index];
  moved[index] = target;
  return _normalizeBlueprintOrder(moved);
}

List<TaskBlueprintNodeDto> _removeBlueprintSubtree(
  List<TaskBlueprintNodeDto> nodes,
  String nodeKey,
) {
  final removed = <String>{nodeKey};
  var changed = true;
  while (changed) {
    changed = false;
    for (final node in nodes) {
      if (node.parentNodeKey != null &&
          removed.contains(node.parentNodeKey) &&
          removed.add(node.nodeKey)) {
        changed = true;
      }
    }
  }
  return _normalizeBlueprintOrder([
    for (final node in nodes)
      if (!removed.contains(node.nodeKey)) node,
  ]);
}

Future<String?> _showTaskIdDialog(BuildContext context) {
  final l10n = AppLocalizations.of(context)!;
  final controller = TextEditingController();
  return showDialog<String>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(l10n.replaceTemplateSnapshotTitle),
      content: TextField(
        controller: controller,
        decoration: InputDecoration(labelText: l10n.sourceTaskIdLabel),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: Text(l10n.cancelButton),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, controller.text.trim()),
          child: Text(l10n.replaceButton),
        ),
      ],
    ),
  );
}

Future<bool> _confirmTemplateDelete(BuildContext context, String name) async {
  final l10n = AppLocalizations.of(context)!;
  return await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: Text(l10n.deleteTemplateDialogTitle(name)),
          content: Text(l10n.deleteTemplateDialogBody),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: Text(l10n.cancelButton),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: Text(l10n.deleteButton),
            ),
          ],
        ),
      ) ??
      false;
}

typedef TaskSeriesDialogValue = ({
  String rrule,
  int startsAt,
  String timeZone,
  bool enabled,
});

Future<TaskSeriesDialogValue?> showTaskSeriesDialog(
  BuildContext context,
  TaskSeriesDto? series,
  String defaultTimeZone,
) async {
  final l10n = AppLocalizations.of(context)!;
  var startsAt =
      series?.startsAt ??
      DateTime.now().add(const Duration(hours: 1)).millisecondsSinceEpoch;
  final initialStart = DateTime.fromMillisecondsSinceEpoch(startsAt);
  var selectedWeekdays = <int>{initialStart.weekday};
  var selectedMonthDay = initialStart.day;
  var weeklyCustomized = false;
  var monthlyCustomized = false;
  final zoneController = TextEditingController(
    text: series?.timeZone ?? defaultTimeZone,
  );
  final controller = TextEditingController(text: series?.rrule ?? 'FREQ=DAILY');
  var preset = series == null ? 'daily' : 'advanced';
  var enabled = series?.enabled ?? true;
  return showDialog<TaskSeriesDialogValue>(
    context: context,
    builder: (context) => StatefulBuilder(
      builder: (context, setState) => AlertDialog(
        title: Text(
          series == null ? l10n.newScheduleTitle : l10n.editScheduleTitle,
        ),
        content: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              DropdownButtonFormField<String>(
                key: const Key('schedule-preset'),
                initialValue: preset,
                isExpanded: true,
                decoration: InputDecoration(
                  labelText: l10n.schedulePresetLabel,
                ),
                items: [
                  DropdownMenuItem(
                    value: 'daily',
                    child: Text(
                      l10n.dailyPreset,
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                  DropdownMenuItem(
                    value: 'weekly',
                    child: Text(
                      l10n.weeklyPreset,
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                  DropdownMenuItem(
                    value: 'monthly',
                    child: Text(
                      l10n.monthlyPreset,
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                  DropdownMenuItem(
                    value: 'advanced',
                    child: Text(
                      l10n.advancedPreset,
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                ],
                onChanged: (value) {
                  if (value == null) return;
                  setState(() {
                    preset = value;
                    controller.text = _presetRule(
                      value,
                      selectedWeekdays,
                      selectedMonthDay,
                      controller.text,
                    );
                  });
                },
              ),
              const SizedBox(height: AppSpacing.md),
              if (preset == 'weekly') ...[
                Wrap(
                  key: const Key('schedule-weekdays'),
                  spacing: AppSpacing.xs,
                  runSpacing: AppSpacing.xs,
                  children: [
                    for (
                      var weekday = DateTime.monday;
                      weekday <= DateTime.sunday;
                      weekday++
                    )
                      FilterChip(
                        key: Key('schedule-weekday-$weekday'),
                        label: Text(
                          DateFormat.E(
                            Localizations.localeOf(context).toLanguageTag(),
                          ).format(DateTime(2024, 1, weekday)),
                        ),
                        selected: selectedWeekdays.contains(weekday),
                        onSelected: (selected) {
                          if (!selected && selectedWeekdays.length == 1) {
                            return;
                          }
                          setState(() {
                            weeklyCustomized = true;
                            if (selected) {
                              selectedWeekdays.add(weekday);
                            } else {
                              selectedWeekdays.remove(weekday);
                            }
                            controller.text = _presetRule(
                              preset,
                              selectedWeekdays,
                              selectedMonthDay,
                              controller.text,
                            );
                          });
                        },
                      ),
                  ],
                ),
                const SizedBox(height: AppSpacing.md),
              ],
              if (preset == 'monthly') ...[
                DropdownButtonFormField<int>(
                  key: const Key('schedule-month-day'),
                  initialValue: selectedMonthDay,
                  isExpanded: true,
                  decoration: InputDecoration(labelText: l10n.monthlyPreset),
                  items: [
                    for (var day = 1; day <= 31; day++)
                      DropdownMenuItem(
                        value: day,
                        child: Text(
                          NumberFormat.decimalPattern(
                            Localizations.localeOf(context).toLanguageTag(),
                          ).format(day),
                        ),
                      ),
                  ],
                  onChanged: (value) {
                    if (value == null) return;
                    setState(() {
                      monthlyCustomized = true;
                      selectedMonthDay = value;
                      controller.text = _presetRule(
                        preset,
                        selectedWeekdays,
                        selectedMonthDay,
                        controller.text,
                      );
                    });
                  },
                ),
                const SizedBox(height: AppSpacing.md),
              ],
              TextField(
                controller: controller,
                enabled: preset == 'advanced',
                decoration: InputDecoration(labelText: l10n.rruleLabel),
              ),
              const SizedBox(height: AppSpacing.md),
              ListTile(
                contentPadding: EdgeInsets.zero,
                title: Text(l10n.scheduleStartsAtLabel),
                subtitle: Text(
                  DateFormat.yMMMd().add_jm().format(
                    DateTime.fromMillisecondsSinceEpoch(startsAt),
                  ),
                ),
                trailing: const Icon(LucideIcons.calendarClock300),
                onTap: () async {
                  final current = DateTime.fromMillisecondsSinceEpoch(startsAt);
                  final date = await showDatePicker(
                    context: context,
                    firstDate: DateTime(2000),
                    lastDate: DateTime(2200),
                    initialDate: current,
                  );
                  if (date == null || !context.mounted) return;
                  final time = await showTimePicker(
                    context: context,
                    initialTime: TimeOfDay.fromDateTime(current),
                  );
                  if (time == null) return;
                  setState(() {
                    final selectedStart = DateTime(
                      date.year,
                      date.month,
                      date.day,
                      time.hour,
                      time.minute,
                    );
                    startsAt = selectedStart.millisecondsSinceEpoch;
                    if (!weeklyCustomized) {
                      selectedWeekdays = {selectedStart.weekday};
                    }
                    if (!monthlyCustomized) {
                      selectedMonthDay = selectedStart.day;
                    }
                    controller.text = _presetRule(
                      preset,
                      selectedWeekdays,
                      selectedMonthDay,
                      controller.text,
                    );
                  });
                },
              ),
              TextField(
                controller: zoneController,
                decoration: InputDecoration(labelText: l10n.timeZoneLabel),
              ),
              if (series != null)
                SwitchListTile(
                  contentPadding: EdgeInsets.zero,
                  title: Text(l10n.scheduleEnabledLabel),
                  value: enabled,
                  onChanged: (value) => setState(() => enabled = value),
                ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: Text(l10n.cancelButton),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, (
              rrule: controller.text.trim(),
              startsAt: startsAt,
              timeZone: zoneController.text.trim(),
              enabled: enabled,
            )),
            child: Text(l10n.saveButton),
          ),
        ],
      ),
    ),
  );
}

String _presetRule(
  String preset,
  Set<int> selectedWeekdays,
  int selectedMonthDay,
  String advancedRule,
) => switch (preset) {
  'daily' => 'FREQ=DAILY',
  'weekly' =>
    'FREQ=WEEKLY;BYDAY=${(selectedWeekdays.toList()..sort()).map(_weekdayCode).join(',')}',
  'monthly' => 'FREQ=MONTHLY;BYMONTHDAY=$selectedMonthDay',
  _ => advancedRule,
};

String _weekdayCode(int weekday) => const {
  DateTime.monday: 'MO',
  DateTime.tuesday: 'TU',
  DateTime.wednesday: 'WE',
  DateTime.thursday: 'TH',
  DateTime.friday: 'FR',
  DateTime.saturday: 'SA',
  DateTime.sunday: 'SU',
}[weekday]!;
