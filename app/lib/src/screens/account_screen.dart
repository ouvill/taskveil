import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:qr_flutter/qr_flutter.dart';
import 'package:taskveil/src/billing/billing_store.dart';
import 'package:taskveil/src/core/bridge_service.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/ui/states.dart';
import 'package:taskveil/src/ui/header_actions.dart';
import 'package:taskveil/src/ui/theme.dart';
import 'package:url_launcher/url_launcher.dart';

class AccountScreen extends ConsumerStatefulWidget {
  const AccountScreen({super.key, this.showBackButton = false});

  final bool showBackButton;

  @override
  ConsumerState<AccountScreen> createState() => _AccountScreenState();
}

enum _RegistrationStep { email, otp, password }

class _AccountScreenState extends ConsumerState<AccountScreen> {
  final _emailController = TextEditingController();
  final _passwordController = TextEditingController();
  final _otpController = TextEditingController();
  final _serverUrlController = TextEditingController();
  bool _registerMode = false;
  bool _busy = false;
  bool _restoringAccountFlow = true;
  bool _recoveryRestoreFailed = false;
  bool _registrationCanCancel = false;
  int? _nextRetryAtMs;
  Timer? _resendTimer;
  _RegistrationStep _registrationStep = _RegistrationStep.email;
  String? _recoveryKey;
  String? _error;

  @override
  void initState() {
    super.initState();
    Future<void>.microtask(_restoreAccountFlow);
  }

  Future<void> _restoreAccountFlow() async {
    if (mounted) {
      setState(() {
        _restoringAccountFlow = true;
        _recoveryRestoreFailed = false;
      });
    }
    try {
      final bridge = ref.read(accountBridgeProvider);
      final recoveryKey = await bridge.accountRegistrationRecoveryKey();
      final registration = await bridge.accountRegistrationState();
      if (!mounted) return;
      setState(() {
        _recoveryKey = recoveryKey;
        if (registration != null) {
          _registerMode = true;
          _emailController.text = registration.email;
          _registrationStep = switch (registration.phase) {
            'email' => _RegistrationStep.email,
            'otp' => _RegistrationStep.otp,
            'password' => _RegistrationStep.password,
            _ => _RegistrationStep.email,
          };
          _registrationCanCancel = registration.canCancel;
          _setNextRetryAt(registration.nextRetryAtMs);
        }
      });
    } catch (_) {
      if (mounted) {
        setState(() => _recoveryRestoreFailed = true);
      }
    } finally {
      if (mounted) {
        setState(() => _restoringAccountFlow = false);
      }
    }
  }

  @override
  void dispose() {
    _emailController.dispose();
    _passwordController.dispose();
    _otpController.dispose();
    _serverUrlController.dispose();
    _resendTimer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final accountAsync = ref.watch(accountProvider);
    final serverUrlAsync = ref.watch(syncServerUrlProvider);
    final syncStatusAsync = ref.watch(syncStatusProvider);
    final billingAsync = ref.watch(billingProvider);

    serverUrlAsync.whenData((serverUrl) {
      if (_serverUrlController.text.isEmpty) {
        _serverUrlController.text = serverUrl;
      }
    });

    return Scaffold(
      body: SafeArea(
        child: accountAsync.when(
          loading: () => const AppLoadingState(),
          error: (error, stackTrace) =>
              AppErrorState(message: l10n.accountLoadFailed),
          data: (account) {
            if (_restoringAccountFlow) {
              return const AppLoadingState();
            }
            if (_recoveryRestoreFailed ||
                (account.recoveryPending && _recoveryKey == null)) {
              return AppEmptyState(
                icon: LucideIcons.keyRound300,
                title: l10n.accountLoadFailed,
                action: FilledButton(
                  onPressed: _restoreAccountFlow,
                  child: Text(l10n.retryButton),
                ),
              );
            }
            return Align(
              alignment: Alignment.topCenter,
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 620),
                child: SingleChildScrollView(
                  padding: const EdgeInsets.fromLTRB(
                    AppSpacing.lg,
                    AppSpacing.lg,
                    AppSpacing.lg,
                    AppSpacing.xl,
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      if (widget.showBackButton) ...[
                        Align(
                          alignment: AlignmentDirectional.centerStart,
                          child: IconButton(
                            tooltip: l10n.backButtonTooltip,
                            alignment: AlignmentDirectional.centerStart,
                            padding: EdgeInsets.zero,
                            constraints: const BoxConstraints.tightFor(
                              width: 48,
                              height: 48,
                            ),
                            onPressed: () {
                              if (context.canPop()) {
                                context.pop();
                              } else {
                                context.go('/menu');
                              }
                            },
                            icon: const Icon(LucideIcons.arrowLeft300),
                          ),
                        ),
                        const SizedBox(height: AppSpacing.sm),
                      ],
                      Row(
                        children: [
                          Expanded(
                            child: Text(
                              l10n.accountTitle,
                              style: theme.textTheme.headlineSmall?.copyWith(
                                color: colorScheme.onSurface,
                                fontSize: 28,
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                          ),
                          if (!widget.showBackButton)
                            const AppHeaderSearchAction(),
                        ],
                      ),
                      const SizedBox(height: AppSpacing.sm),
                      Text(
                        l10n.accountSubtitle,
                        style: theme.textTheme.bodyMedium?.copyWith(
                          color: colorScheme.onSurfaceVariant,
                        ),
                      ),
                      const SizedBox(height: AppSpacing.md),
                      const _AccountAccentLine(),
                      const SizedBox(height: AppSpacing.xl),
                      if (_recoveryKey != null) ...[
                        SelectableText(
                          _recoveryKey!,
                          key: const ValueKey('account-recovery-key'),
                          style: theme.textTheme.bodyLarge,
                        ),
                        const SizedBox(height: AppSpacing.md),
                        FilledButton(
                          onPressed: _busy ? null : _acknowledgeRecoveryKey,
                          child: Text(l10n.accountRecoveryAcknowledgeButton),
                        ),
                        const SizedBox(height: AppSpacing.lg),
                      ],
                      if (account.loggedIn)
                        _SignedInSection(
                          account: account,
                          syncStatusAsync: syncStatusAsync,
                          billingAsync: billingAsync,
                          busy: _busy,
                          onLogout: _logout,
                          onSyncNow: _syncNow,
                          onVerifyOrganization: _showOrganizationVerification,
                        )
                      else
                        _SignedOutSection(
                          registerMode: _registerMode,
                          busy: _busy,
                          registrationStep: _registrationStep,
                          emailController: _emailController,
                          passwordController: _passwordController,
                          otpController: _otpController,
                          onModeChanged: (registerMode) {
                            setState(() {
                              _registerMode = registerMode;
                              _registrationStep = _RegistrationStep.email;
                              _passwordController.clear();
                              _otpController.clear();
                              _recoveryKey = null;
                              _error = null;
                            });
                          },
                          onSubmit: _submit,
                          onResend: _resendCode,
                          resendWaitSeconds: _resendWaitSeconds,
                          canCancel: _registrationCanCancel,
                          onCancel: _cancelRegistration,
                        ),
                      if (_error != null) ...[
                        const SizedBox(height: AppSpacing.md),
                        Row(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Icon(
                              LucideIcons.circleAlert300,
                              size: 18,
                              color: colorScheme.error,
                            ),
                            const SizedBox(width: AppSpacing.sm),
                            Expanded(
                              child: Text(
                                _error!,
                                style: TextStyle(color: colorScheme.error),
                              ),
                            ),
                          ],
                        ),
                      ],
                      const SizedBox(height: AppSpacing.xl),
                      Divider(color: colorScheme.outlineVariant),
                      const SizedBox(height: AppSpacing.lg),
                      _ServerUrlSection(
                        controller: _serverUrlController,
                        busy: _busy,
                        onSave: _saveServerUrl,
                      ),
                    ],
                  ),
                ),
              ),
            );
          },
        ),
      ),
    );
  }

  Future<void> _saveServerUrl() async {
    final l10n = AppLocalizations.of(context)!;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await ref
          .read(syncServerUrlProvider.notifier)
          .setServerUrl(_serverUrlController.text.trim());
    } catch (_) {
      setState(() => _error = l10n.accountRequestFailed);
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  Future<void> _submit() async {
    final l10n = AppLocalizations.of(context)!;
    setState(() {
      _busy = true;
      _error = null;
      _recoveryKey = null;
    });
    try {
      final notifier = ref.read(accountProvider.notifier);
      if (!_registerMode) {
        final result = await notifier.login(
          email: _emailController.text.trim(),
          password: _passwordController.text,
          serverUrl: _serverUrlController.text.trim(),
        );
        _passwordController.clear();
        if (mounted) {
          setState(() => _recoveryKey = result.recoveryKey);
        }
      } else {
        switch (_registrationStep) {
          case _RegistrationStep.email:
            _passwordController.clear();
            final pending = await notifier.registrationBegin(
              email: _emailController.text.trim(),
              serverUrl: _serverUrlController.text.trim(),
            );
            if (mounted) {
              setState(() {
                _registrationStep = _RegistrationStep.otp;
                _registrationCanCancel = true;
                _setNextRetryAt(pending.nextRetryAtMs);
              });
            }
            break;
          case _RegistrationStep.otp:
            await notifier.registrationVerifyOtp(_otpController.text.trim());
            _otpController.clear();
            if (mounted) {
              setState(() {
                _registrationStep = _RegistrationStep.password;
                _registrationCanCancel = false;
              });
            }
            break;
          case _RegistrationStep.password:
            final result = await notifier.registrationComplete(
              password: _passwordController.text,
            );
            _passwordController.clear();
            if (mounted) {
              setState(() => _recoveryKey = result.recoveryKey);
            }
            break;
        }
      }
    } catch (_) {
      if (mounted) {
        setState(() => _error = l10n.accountRequestFailed);
      }
    } finally {
      if (mounted) {
        setState(() {
          _busy = false;
        });
      }
    }
  }

  Future<void> _resendCode() async {
    final l10n = AppLocalizations.of(context)!;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final pending = await ref
          .read(accountProvider.notifier)
          .registrationResend();
      if (mounted) {
        setState(() => _setNextRetryAt(pending.nextRetryAtMs));
      }
    } catch (_) {
      if (mounted) {
        setState(() => _error = l10n.accountRequestFailed);
      }
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  int get _resendWaitSeconds {
    final nextRetryAtMs = _nextRetryAtMs;
    if (nextRetryAtMs == null) return 0;
    final remaining = nextRetryAtMs - DateTime.now().millisecondsSinceEpoch;
    return remaining <= 0 ? 0 : (remaining / 1000).ceil();
  }

  void _setNextRetryAt(int? nextRetryAtMs) {
    _nextRetryAtMs = nextRetryAtMs;
    _resendTimer?.cancel();
    if (_resendWaitSeconds == 0) return;
    _resendTimer = Timer.periodic(const Duration(seconds: 1), (timer) {
      if (!mounted || _resendWaitSeconds == 0) {
        timer.cancel();
      }
      if (mounted) setState(() {});
    });
  }

  Future<void> _cancelRegistration() async {
    final l10n = AppLocalizations.of(context)!;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await ref.read(accountProvider.notifier).registrationCancel();
      if (mounted) {
        setState(() {
          _registerMode = false;
          _registrationStep = _RegistrationStep.email;
          _registrationCanCancel = false;
          _setNextRetryAt(null);
          _passwordController.clear();
          _otpController.clear();
        });
      }
    } catch (_) {
      if (mounted) {
        setState(() => _error = l10n.accountRequestFailed);
      }
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  Future<void> _acknowledgeRecoveryKey() async {
    setState(() => _busy = true);
    try {
      await ref.read(accountProvider.notifier).registrationAckRecoveryKey();
      if (mounted) {
        setState(() {
          _recoveryKey = null;
          _registrationStep = _RegistrationStep.email;
          _registrationCanCancel = false;
        });
      }
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  Future<void> _logout() async {
    final l10n = AppLocalizations.of(context)!;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await ref.read(accountProvider.notifier).logout();
      if (mounted) {
        setState(() => _recoveryKey = null);
      }
    } catch (_) {
      if (mounted) {
        setState(() => _error = l10n.accountRequestFailed);
      }
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  Future<void> _syncNow() async {
    await ref.read(syncStatusProvider.notifier).syncNow();
  }

  Future<void> _showOrganizationVerification() async {
    await showDialog<void>(
      context: context,
      builder: (context) =>
          _OrganizationSafetyDialog(bridge: ref.read(accountBridgeProvider)),
    );
  }
}

class _AccountAccentLine extends StatelessWidget {
  const _AccountAccentLine();

  @override
  Widget build(BuildContext context) => Align(
    alignment: AlignmentDirectional.centerStart,
    child: Container(
      width: 36,
      height: 2,
      color: Theme.of(context).colorScheme.primary,
    ),
  );
}

class _AccountSectionLabel extends StatelessWidget {
  const _AccountSectionLabel(this.label);

  final String label;

  @override
  Widget build(BuildContext context) => Text(
    label,
    style: Theme.of(context).textTheme.labelSmall?.copyWith(
      color: Theme.of(context).colorScheme.onSurfaceVariant,
      letterSpacing: 1.35,
    ),
  );
}

class _AccountActionRow extends StatelessWidget {
  const _AccountActionRow({
    super.key,
    required this.icon,
    required this.label,
    required this.onPressed,
    this.destructive = false,
  });

  final IconData icon;
  final String label;
  final VoidCallback? onPressed;
  final bool destructive;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final foreground = destructive ? colorScheme.error : colorScheme.onSurface;
    return InkWell(
      onTap: onPressed,
      child: ConstrainedBox(
        constraints: const BoxConstraints(minHeight: 54),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: AppSpacing.xs),
          child: Row(
            children: [
              Icon(icon, size: 20, color: foreground),
              const SizedBox(width: AppSpacing.md),
              Expanded(
                child: Text(
                  label,
                  style: theme.textTheme.bodyLarge?.copyWith(
                    color: foreground,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
              Icon(
                LucideIcons.chevronRight300,
                size: 19,
                color: destructive
                    ? colorScheme.error
                    : colorScheme.onSurfaceVariant,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _ServerUrlSection extends StatelessWidget {
  const _ServerUrlSection({
    required this.controller,
    required this.busy,
    required this.onSave,
  });

  final TextEditingController controller;
  final bool busy;
  final VoidCallback onSave;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _AccountSectionLabel(l10n.accountConnectionTitle),
        const SizedBox(height: AppSpacing.sm),
        Text(l10n.accountServerUrlLabel, style: theme.textTheme.titleMedium),
        const SizedBox(height: AppSpacing.xs),
        Text(
          l10n.accountConnectionBody,
          style: theme.textTheme.bodySmall?.copyWith(
            color: colorScheme.onSurfaceVariant,
          ),
        ),
        const SizedBox(height: AppSpacing.md),
        TextField(
          controller: controller,
          keyboardType: TextInputType.url,
          textInputAction: TextInputAction.done,
          onSubmitted: busy ? null : (_) => onSave(),
          decoration: InputDecoration(
            hintText: defaultSyncServerUrl,
            suffixIcon: IconButton(
              tooltip: l10n.accountSaveServerUrlTooltip,
              onPressed: busy ? null : onSave,
              icon: const Icon(LucideIcons.save300),
            ),
          ),
        ),
      ],
    );
  }
}

class _SignedOutSection extends StatelessWidget {
  const _SignedOutSection({
    required this.registerMode,
    required this.busy,
    required this.registrationStep,
    required this.emailController,
    required this.passwordController,
    required this.otpController,
    required this.onModeChanged,
    required this.onSubmit,
    required this.onResend,
    required this.resendWaitSeconds,
    required this.canCancel,
    required this.onCancel,
  });

  final bool registerMode;
  final bool busy;
  final _RegistrationStep registrationStep;
  final TextEditingController emailController;
  final TextEditingController passwordController;
  final TextEditingController otpController;
  final ValueChanged<bool> onModeChanged;
  final VoidCallback onSubmit;
  final VoidCallback onResend;
  final int resendWaitSeconds;
  final bool canCancel;
  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _AccountSectionLabel(l10n.accountPrivateSectionTitle),
        const SizedBox(height: AppSpacing.md),
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Icon(
                LucideIcons.shieldCheck300,
                size: 22,
                color: colorScheme.primary,
              ),
            ),
            const SizedBox(width: AppSpacing.md),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    l10n.accountPrivateTitle,
                    style: theme.textTheme.titleMedium,
                  ),
                  const SizedBox(height: AppSpacing.xs),
                  Text(
                    l10n.accountPrivateBody,
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: colorScheme.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
        const SizedBox(height: AppSpacing.lg),
        Divider(color: colorScheme.outlineVariant),
        const SizedBox(height: AppSpacing.md),
        Row(
          children: [
            Expanded(
              child: _AccountModeButton(
                label: l10n.accountLoginTab,
                selected: !registerMode,
                onPressed: busy ? null : () => onModeChanged(false),
              ),
            ),
            const SizedBox(width: AppSpacing.sm),
            Expanded(
              child: _AccountModeButton(
                label: l10n.accountRegisterTab,
                selected: registerMode,
                onPressed: busy ? null : () => onModeChanged(true),
              ),
            ),
          ],
        ),
        const SizedBox(height: AppSpacing.lg),
        if (!registerMode || registrationStep == _RegistrationStep.email) ...[
          TextField(
            controller: emailController,
            keyboardType: TextInputType.emailAddress,
            autofillHints: const [AutofillHints.email],
            decoration: InputDecoration(labelText: l10n.accountEmailLabel),
          ),
          const SizedBox(height: AppSpacing.md),
        ],
        if (!registerMode ||
            registrationStep == _RegistrationStep.password) ...[
          TextField(
            controller: passwordController,
            obscureText: true,
            autofillHints: const [AutofillHints.password],
            decoration: InputDecoration(labelText: l10n.accountPasswordLabel),
          ),
          const SizedBox(height: AppSpacing.md),
        ],
        if (registerMode && registrationStep == _RegistrationStep.otp) ...[
          Semantics(
            liveRegion: true,
            child: Text(
              l10n.accountVerificationPending,
              key: const ValueKey('account-verification-pending'),
              style: theme.textTheme.bodySmall?.copyWith(
                color: colorScheme.onSurfaceVariant,
              ),
            ),
          ),
          const SizedBox(height: AppSpacing.md),
          TextField(
            controller: otpController,
            keyboardType: TextInputType.number,
            autofillHints: const [AutofillHints.oneTimeCode],
            maxLength: 8,
            decoration: InputDecoration(
              labelText: l10n.accountVerificationCodeLabel,
            ),
          ),
          const SizedBox(height: AppSpacing.sm),
        ],
        if (registerMode && canCancel) ...[
          const SizedBox(height: AppSpacing.sm),
          TextButton(
            onPressed: busy ? null : onCancel,
            child: Text(l10n.accountCancelRegistrationButton),
          ),
        ],
        const SizedBox(height: AppSpacing.lg),
        FilledButton.icon(
          onPressed: busy ? null : onSubmit,
          icon: Icon(
            registerMode ? LucideIcons.userPlus300 : LucideIcons.logIn300,
          ),
          label: Text(
            !registerMode
                ? l10n.accountLoginButton
                : switch (registrationStep) {
                    _RegistrationStep.email => l10n.accountRegisterButton,
                    _RegistrationStep.otp => l10n.accountVerifyCodeButton,
                    _RegistrationStep.password => l10n.accountSetPasswordButton,
                  },
          ),
          style: FilledButton.styleFrom(
            minimumSize: const Size.fromHeight(50),
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(8),
            ),
          ),
        ),
        if (registerMode && registrationStep == _RegistrationStep.otp) ...[
          const SizedBox(height: AppSpacing.md),
          if (resendWaitSeconds > 0) ...[
            Text(
              l10n.accountResendAvailableIn(resendWaitSeconds),
              textAlign: TextAlign.center,
              style: theme.textTheme.bodySmall?.copyWith(
                color: colorScheme.onSurfaceVariant,
              ),
            ),
            const SizedBox(height: AppSpacing.xs),
          ],
          TextButton.icon(
            onPressed: busy || resendWaitSeconds > 0 ? null : onResend,
            icon: const Icon(LucideIcons.refreshCw300),
            label: Text(l10n.accountResendCodeButton),
          ),
        ],
      ],
    );
  }
}

class _AccountModeButton extends StatelessWidget {
  const _AccountModeButton({
    required this.label,
    required this.selected,
    required this.onPressed,
  });

  final String label;
  final bool selected;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    return DecoratedBox(
      decoration: BoxDecoration(
        border: Border(
          bottom: BorderSide(
            color: selected ? colorScheme.primary : colorScheme.outlineVariant,
            width: selected ? 2 : 1,
          ),
        ),
      ),
      child: TextButton(
        onPressed: onPressed,
        style: TextButton.styleFrom(
          foregroundColor: selected
              ? colorScheme.primary
              : colorScheme.onSurfaceVariant,
          shape: const RoundedRectangleBorder(),
        ),
        child: Text(
          label,
          style: theme.textTheme.labelLarge?.copyWith(
            fontWeight: selected ? FontWeight.w700 : FontWeight.w500,
          ),
        ),
      ),
    );
  }
}

class _SignedInSection extends StatelessWidget {
  const _SignedInSection({
    required this.account,
    required this.syncStatusAsync,
    required this.billingAsync,
    required this.busy,
    required this.onLogout,
    required this.onSyncNow,
    required this.onVerifyOrganization,
  });

  final AccountSessionStateDto account;
  final AsyncValue<SyncStatusDto> syncStatusAsync;
  final AsyncValue<BillingUiState?> billingAsync;
  final bool busy;
  final VoidCallback onLogout;
  final VoidCallback onSyncNow;
  final VoidCallback onVerifyOrganization;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;
    final email = account.email ?? '';
    final initial = email.trim().isEmpty ? '?' : email.trim()[0].toUpperCase();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _AccountSectionLabel(l10n.accountPrivateSectionTitle),
        const SizedBox(height: AppSpacing.md),
        Row(
          children: [
            Container(
              width: 54,
              height: 54,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                border: Border.all(color: colorScheme.primary, width: 1.5),
              ),
              child: Text(
                initial,
                style: theme.textTheme.titleLarge?.copyWith(
                  color: colorScheme.primary,
                ),
              ),
            ),
            const SizedBox(width: AppSpacing.md),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    email,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.titleMedium,
                  ),
                  const SizedBox(height: AppSpacing.xs),
                  Row(
                    children: [
                      Icon(
                        LucideIcons.lockKeyhole300,
                        size: 14,
                        color: colorScheme.primary,
                      ),
                      const SizedBox(width: AppSpacing.xs),
                      Expanded(
                        child: Text(
                          l10n.accountEncryptionStatus,
                          style: theme.textTheme.bodySmall?.copyWith(
                            color: colorScheme.onSurfaceVariant,
                          ),
                        ),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ],
        ),
        const SizedBox(height: AppSpacing.xl),
        _SyncStatusSection(
          syncStatusAsync: syncStatusAsync,
          busy: busy,
          onSyncNow: onSyncNow,
        ),
        const SizedBox(height: AppSpacing.lg),
        Divider(color: colorScheme.outlineVariant),
        const SizedBox(height: AppSpacing.lg),
        _BillingSection(billingAsync: billingAsync),
        const SizedBox(height: AppSpacing.lg),
        Divider(color: colorScheme.outlineVariant),
        const SizedBox(height: AppSpacing.lg),
        _AccountSectionLabel(l10n.accountSecurityTitle),
        const SizedBox(height: AppSpacing.sm),
        _AccountActionRow(
          key: const ValueKey('organization-safety-open'),
          onPressed: busy ? null : onVerifyOrganization,
          icon: LucideIcons.shieldCheck300,
          label: l10n.organizationSafetyOpenButton,
        ),
        Divider(color: colorScheme.outlineVariant),
        _AccountActionRow(
          key: const ValueKey('account-logout'),
          onPressed: busy ? null : onLogout,
          icon: LucideIcons.logOut300,
          label: l10n.accountLogoutButton,
          destructive: true,
        ),
      ],
    );
  }
}

class _BillingSection extends ConsumerWidget {
  const _BillingSection({required this.billingAsync});

  final AsyncValue<BillingUiState?> billingAsync;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final l10n = AppLocalizations.of(context)!;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _AccountSectionLabel(l10n.billingTitle),
        const SizedBox(height: AppSpacing.sm),
        billingAsync.when(
          loading: () => const LinearProgressIndicator(minHeight: 2),
          error: (_, _) => Text(l10n.billingUnavailable),
          data: (value) {
            if (value == null) return Text(l10n.billingStatusFree);
            final entitlement = value.entitlement;
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    Icon(
                      entitlement.syncAllowed
                          ? LucideIcons.badgeCheck300
                          : LucideIcons.badge300,
                      size: 18,
                      semanticLabel: _billingStatus(l10n, entitlement.status),
                    ),
                    const SizedBox(width: AppSpacing.sm),
                    Expanded(
                      child: Text(_billingStatus(l10n, entitlement.status)),
                    ),
                  ],
                ),
                if (!entitlement.syncAllowed) ...[
                  const SizedBox(height: AppSpacing.sm),
                  Text(
                    l10n.billingSubscriptionBody,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                  const SizedBox(height: AppSpacing.md),
                  for (final product in value.products) ...[
                    _BillingProductTile(
                      product: product,
                      busy: value.busy,
                      onPurchase: () => ref
                          .read(billingProvider.notifier)
                          .purchase(product.identifier),
                    ),
                    const SizedBox(height: AppSpacing.sm),
                  ],
                ],
                if (value.lastOutcome case final outcome?) ...[
                  const SizedBox(height: AppSpacing.sm),
                  Text(_billingOutcome(l10n, outcome)),
                ],
                const SizedBox(height: AppSpacing.sm),
                Wrap(
                  spacing: AppSpacing.sm,
                  runSpacing: AppSpacing.xs,
                  children: [
                    TextButton.icon(
                      onPressed: value.busy
                          ? null
                          : () => ref.read(billingProvider.notifier).restore(),
                      icon: const Icon(LucideIcons.refreshCcw300),
                      label: Text(l10n.billingRestoreButton),
                    ),
                    if (entitlement.syncAllowed)
                      TextButton.icon(
                        onPressed: value.busy
                            ? null
                            : () async {
                                final url = await ref
                                    .read(billingStoreProvider)
                                    .managementUrl();
                                if (url != null) {
                                  await launchUrl(
                                    url,
                                    mode: LaunchMode.externalApplication,
                                  );
                                }
                              },
                        icon: const Icon(LucideIcons.externalLink300),
                        label: Text(l10n.billingManageButton),
                      ),
                  ],
                ),
              ],
            );
          },
        ),
      ],
    );
  }

  static String _billingStatus(AppLocalizations l10n, String status) =>
      switch (status) {
        'trial' => l10n.billingStatusTrial,
        'active' => l10n.billingStatusActive,
        'grace' => l10n.billingStatusGrace,
        'expired' => l10n.billingStatusExpired,
        'revoked' => l10n.billingStatusRevoked,
        _ => l10n.billingStatusFree,
      };

  static String _billingOutcome(
    AppLocalizations l10n,
    BillingPurchaseOutcome outcome,
  ) => switch (outcome) {
    BillingPurchaseOutcome.purchased => l10n.billingRestored,
    BillingPurchaseOutcome.cancelled => l10n.billingCancelled,
    BillingPurchaseOutcome.pending => l10n.billingPending,
    BillingPurchaseOutcome.failed => l10n.billingFailed,
  };
}

class _BillingProductTile extends StatelessWidget {
  const _BillingProductTile({
    required this.product,
    required this.busy,
    required this.onPurchase,
  });

  final BillingProduct product;
  final bool busy;
  final VoidCallback onPurchase;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final period = product.isAnnual
        ? l10n.billingYearlyLabel
        : l10n.billingMonthlyLabel;
    return Semantics(
      container: true,
      explicitChildNodes: true,
      label: l10n.billingPriceSemantics(period, product.price),
      child: DecoratedBox(
        decoration: BoxDecoration(
          border: Border.all(
            color: Theme.of(context).colorScheme.outlineVariant,
          ),
          borderRadius: BorderRadius.circular(10),
        ),
        child: Padding(
          padding: const EdgeInsets.all(AppSpacing.md),
          child: Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(period, style: Theme.of(context).textTheme.titleSmall),
                    const SizedBox(height: AppSpacing.xs),
                    Text(product.price),
                  ],
                ),
              ),
              FilledButton(
                onPressed: busy ? null : onPurchase,
                child: Text(l10n.billingPurchaseButton),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _OrganizationSafetyDialog extends StatefulWidget {
  const _OrganizationSafetyDialog({required this.bridge});

  final AccountBridgePort bridge;

  @override
  State<_OrganizationSafetyDialog> createState() =>
      _OrganizationSafetyDialogState();
}

class _OrganizationSafetyDialogState extends State<_OrganizationSafetyDialog> {
  final _tenantController = TextEditingController();
  final _memberController = TextEditingController();
  OrganizationSafetyStateDto? _state;
  bool _comparedOutOfBand = false;
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _tenantController.dispose();
    _memberController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final state = _state;
    return AlertDialog(
      title: Text(l10n.organizationSafetyTitle),
      content: SizedBox(
        width: 440,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(l10n.organizationSafetyBody),
              const SizedBox(height: AppSpacing.md),
              TextField(
                key: const ValueKey('organization-tenant-id'),
                controller: _tenantController,
                enabled: !_busy,
                decoration: InputDecoration(
                  labelText: l10n.organizationTenantIdLabel,
                ),
              ),
              const SizedBox(height: AppSpacing.sm),
              TextField(
                key: const ValueKey('organization-member-id'),
                controller: _memberController,
                enabled: !_busy,
                decoration: InputDecoration(
                  labelText: l10n.organizationMemberIdLabel,
                ),
              ),
              const SizedBox(height: AppSpacing.md),
              if (state == null)
                FilledButton(
                  key: const ValueKey('organization-safety-load'),
                  onPressed: _busy ? null : _load,
                  child: Text(l10n.organizationSafetyLoadButton),
                )
              else ...[
                Center(
                  child: QrImageView.withQr(
                    key: const ValueKey('organization-safety-qr'),
                    qr: QrCode.fromUint8List(
                      data: base64Decode(state.qrPayload),
                      errorCorrectLevel: QrErrorCorrectLevel.M,
                    ),
                    size: 180,
                    semanticsLabel: l10n.organizationSafetyQrSemantics,
                  ),
                ),
                const SizedBox(height: AppSpacing.md),
                SelectableText(
                  _groupSafetyNumber(state.decimal),
                  key: const ValueKey('organization-safety-number'),
                  textAlign: TextAlign.center,
                  style: Theme.of(context).textTheme.titleMedium?.copyWith(
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
                const SizedBox(height: AppSpacing.md),
                Text(
                  state.verificationState == 'verified'
                      ? l10n.organizationSafetyVerified
                      : l10n.organizationSafetyUnverified,
                  textAlign: TextAlign.center,
                ),
                CheckboxListTile(
                  key: const ValueKey('organization-safety-compared'),
                  contentPadding: EdgeInsets.zero,
                  value: _comparedOutOfBand,
                  onChanged: _busy
                      ? null
                      : (value) =>
                            setState(() => _comparedOutOfBand = value ?? false),
                  title: Text(l10n.organizationSafetyComparedOutOfBand),
                  controlAffinity: ListTileControlAffinity.leading,
                ),
                FilledButton.icon(
                  key: const ValueKey('organization-safety-confirm'),
                  onPressed: _busy || !_comparedOutOfBand ? null : _confirm,
                  icon: const Icon(LucideIcons.badgeCheck300),
                  label: Text(l10n.organizationSafetyConfirmButton),
                ),
              ],
              if (_error != null) ...[
                const SizedBox(height: AppSpacing.sm),
                Text(
                  _error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ],
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: _busy ? null : () => Navigator.of(context).pop(),
          child: Text(l10n.cancelButton),
        ),
      ],
    );
  }

  Future<void> _load() async {
    await _run(() async {
      final state = await widget.bridge.organizationSafetyNumber(
        tenantId: _tenantController.text.trim(),
        memberUserId: _memberController.text.trim(),
      );
      if (mounted) setState(() => _state = state);
    });
  }

  Future<void> _confirm() async {
    final current = _state;
    if (current == null) return;
    await _run(() async {
      final state = await widget.bridge.confirmOrganizationSafetyNumber(
        tenantId: _tenantController.text.trim(),
        memberUserId: _memberController.text.trim(),
        digest: current.digest,
      );
      if (mounted) setState(() => _state = state);
    });
  }

  Future<void> _run(Future<void> Function() operation) async {
    final l10n = AppLocalizations.of(context)!;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await operation();
    } catch (_) {
      if (mounted) setState(() => _error = l10n.organizationSafetyFailed);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  String _groupSafetyNumber(String value) {
    final chunks = <String>[];
    for (var offset = 0; offset < value.length; offset += 5) {
      final end = (offset + 5).clamp(0, value.length);
      chunks.add(value.substring(offset, end));
    }
    return chunks.join(' ');
  }
}

class _SyncStatusSection extends StatelessWidget {
  const _SyncStatusSection({
    required this.syncStatusAsync,
    required this.busy,
    required this.onSyncNow,
  });

  final AsyncValue<SyncStatusDto> syncStatusAsync;
  final bool busy;
  final VoidCallback onSyncNow;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    final status = syncStatusAsync.value;
    final running = status?.running ?? false;
    final statusText = switch (status) {
      null => l10n.accountSyncIdle,
      SyncStatusDto(loggedIn: false) => l10n.accountSyncNotSignedIn,
      SyncStatusDto(running: true) => l10n.accountSyncRunning,
      SyncStatusDto(lastError: final error?) when error.isNotEmpty =>
        l10n.accountSyncFailed,
      _ => l10n.accountSyncIdle,
    };
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _AccountSectionLabel(l10n.accountSyncTitle),
        const SizedBox(height: AppSpacing.sm),
        Row(
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Container(
                        width: 8,
                        height: 8,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: status?.lastError?.isNotEmpty == true
                              ? Theme.of(context).colorScheme.error
                              : Theme.of(context).colorScheme.primary,
                        ),
                      ),
                      const SizedBox(width: AppSpacing.sm),
                      Expanded(
                        child: Text(
                          statusText,
                          style: Theme.of(context).textTheme.titleMedium,
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: AppSpacing.xs),
                  Text(
                    l10n.accountSyncLastSuccess(
                      _formatSyncTime(context, status?.lastSuccessAt),
                    ),
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ],
              ),
            ),
            const SizedBox(width: AppSpacing.sm),
            OutlinedButton.icon(
              onPressed: busy || running ? null : onSyncNow,
              icon: const Icon(LucideIcons.refreshCw300),
              label: Text(l10n.accountSyncNowButton),
              style: OutlinedButton.styleFrom(
                side: BorderSide.none,
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(8),
                ),
              ),
            ),
          ],
        ),
      ],
    );
  }

  String _formatSyncTime(BuildContext context, int? milliseconds) {
    final l10n = AppLocalizations.of(context)!;
    if (milliseconds == null) {
      return l10n.accountSyncNever;
    }
    final dateTime = DateTime.fromMillisecondsSinceEpoch(
      milliseconds,
    ).toLocal();
    final material = MaterialLocalizations.of(context);
    return '${material.formatShortDate(dateTime)} '
        '${material.formatTimeOfDay(TimeOfDay.fromDateTime(dateTime))}';
  }
}
