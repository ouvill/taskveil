import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/ui/dialogs.dart';
import 'package:taskveil/src/ui/bridge_error_messages.dart';
import 'package:taskveil/src/ui/header_actions.dart';
import 'package:taskveil/src/ui/states.dart';
import 'package:taskveil/src/ui/theme.dart';

/// List management and switching surface.
///
/// The app now opens to a task-first home surface. This screen remains the
/// quiet place to switch lists and create additional containers.
class ListsScreen extends ConsumerWidget {
  const ListsScreen({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = AppLocalizations.of(context)!;
    final listsAsync = ref.watch(listsProvider);
    final archivedListsAsync = ref.watch(archivedListsProvider);

    return Scaffold(
      body: SafeArea(
        child: listsAsync.when(
          loading: () => const AppLoadingState(),
          error: (error, stackTrace) => AppErrorState(
            message: l10n.failedToLoadLists(bridgeErrorMessage(l10n, error)),
          ),
          data: (lists) {
            return archivedListsAsync.when(
              loading: () => _ListsManagementView(
                lists: lists,
                archivedLists: const [],
                onCreateList: () => _createList(context, ref),
              ),
              error: (error, stackTrace) => AppErrorState(
                message: l10n.failedToLoadLists(
                  bridgeErrorMessage(l10n, error),
                ),
              ),
              data: (archivedLists) => _ListsManagementView(
                lists: lists,
                archivedLists: archivedLists,
                onCreateList: () => _createList(context, ref),
              ),
            );
          },
        ),
      ),
    );
  }

  Future<void> _createList(BuildContext context, WidgetRef ref) async {
    final l10n = AppLocalizations.of(context)!;
    final name = await showAppTextInputDialog(
      context: context,
      title: l10n.newListTitle,
      label: l10n.nameLabel,
      cancelLabel: l10n.cancelButton,
      submitLabel: l10n.createButton,
    );
    if (name == null || name.trim().isEmpty) {
      return;
    }
    await ref.read(listsProvider.notifier).createList(name.trim());
  }
}

class _ListsManagementView extends StatefulWidget {
  const _ListsManagementView({
    required this.lists,
    required this.archivedLists,
    required this.onCreateList,
  });

  final List<ListDto> lists;
  final List<ListDto> archivedLists;
  final VoidCallback onCreateList;

  @override
  State<_ListsManagementView> createState() => _ListsManagementViewState();
}

class _ListsManagementViewState extends State<_ListsManagementView> {
  bool _archivedExpanded = false;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;

    return Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 760),
        child: ListView(
          padding: const EdgeInsets.fromLTRB(
            AppSpacing.md,
            AppSpacing.lg,
            AppSpacing.md,
            AppSpacing.xl,
          ),
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    l10n.listsTitle,
                    style: theme.textTheme.headlineSmall?.copyWith(
                      color: colorScheme.onSurface,
                      fontSize: 28,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                const AppHeaderSearchAction(),
              ],
            ),
            const SizedBox(height: AppSpacing.lg),
            Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (widget.lists.isEmpty)
                  Padding(
                    padding: const EdgeInsets.all(AppSpacing.lg),
                    child: AppEmptyState(
                      icon: LucideIcons.listTodo300,
                      title: l10n.listsEmptyTitle,
                      body: l10n.listsEmptyBody,
                    ),
                  )
                else
                  for (var index = 0; index < widget.lists.length; index += 1)
                    _ListManagementRow(
                      icon: LucideIcons.circle300,
                      color: _listAccent(colorScheme, index),
                      title: widget.lists[index].name,
                      onTap: () => context.push(
                        '/lists/${widget.lists[index].id}/tasks',
                      ),
                    ),
                const SizedBox(height: AppSpacing.sm),
                _ListManagementRow(
                  icon: LucideIcons.copyPlus300,
                  color: colorScheme.secondary,
                  title: l10n.templatesTitle,
                  onTap: () => context.push('/templates'),
                ),
                _ListManagementRow(
                  icon: LucideIcons.plus300,
                  color: colorScheme.primary,
                  title: l10n.homeNewListButton,
                  onTap: widget.onCreateList,
                ),
                if (widget.archivedLists.isNotEmpty) ...[
                  const SizedBox(height: AppSpacing.lg),
                  _ArchivedListsHeader(
                    count: widget.archivedLists.length,
                    expanded: _archivedExpanded,
                    onTap: () =>
                        setState(() => _archivedExpanded = !_archivedExpanded),
                  ),
                  if (_archivedExpanded)
                    for (
                      var index = 0;
                      index < widget.archivedLists.length;
                      index += 1
                    )
                      _ListManagementRow(
                        icon: LucideIcons.archive300,
                        color: colorScheme.onSurfaceVariant,
                        title: widget.archivedLists[index].name,
                        onTap: () => context.push(
                          '/lists/${widget.archivedLists[index].id}/tasks',
                        ),
                      ),
                ],
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class _ArchivedListsHeader extends StatelessWidget {
  const _ArchivedListsHeader({
    required this.count,
    required this.expanded,
    required this.onTap,
  });

  final int count;
  final bool expanded;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final tooltip = expanded
        ? l10n.hideArchivedListsTooltip
        : l10n.showArchivedListsTooltip;
    return Tooltip(
      message: tooltip,
      child: Semantics(
        button: true,
        label: tooltip,
        child: InkWell(
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: AppSpacing.md,
              vertical: AppSpacing.sm,
            ),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    l10n.archivedListsSectionTitle(count),
                    style: theme.textTheme.titleMedium?.copyWith(
                      color: colorScheme.onSurfaceVariant,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                SizedBox(
                  width: 48,
                  height: 48,
                  child: Center(
                    child: Icon(
                      expanded
                          ? LucideIcons.chevronUp300
                          : LucideIcons.chevronDown300,
                      color: colorScheme.onSurfaceVariant,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _ListManagementRow extends StatelessWidget {
  const _ListManagementRow({
    required this.icon,
    required this.color,
    required this.title,
    required this.onTap,
  });

  final IconData icon;
  final Color color;
  final String title;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    return InkWell(
      onTap: onTap,
      child: DecoratedBox(
        decoration: BoxDecoration(
          border: Border(
            bottom: BorderSide(color: colorScheme.outlineVariant, width: 0.7),
          ),
        ),
        child: ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 56),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: AppSpacing.xs),
            child: Row(
              children: [
                if (icon == LucideIcons.circle300)
                  Container(
                    width: 3,
                    height: 20,
                    decoration: BoxDecoration(
                      color: color,
                      borderRadius: BorderRadius.circular(2),
                    ),
                  )
                else
                  SizedBox.square(
                    dimension: 24,
                    child: Icon(icon, color: color, size: 18),
                  ),
                const SizedBox(width: 13),
                Expanded(
                  child: Text(
                    title,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    softWrap: true,
                    style: theme.textTheme.titleMedium?.copyWith(
                      color: colorScheme.onSurface,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

Color _listAccent(ColorScheme colorScheme, int index) {
  final accents = [
    colorScheme.primary,
    const Color(0xFF6FA17B),
    const Color(0xFFEDB73E),
    const Color(0xFF6F65B5),
    const Color(0xFFE8755A),
  ];
  return accents[index % accents.length];
}
