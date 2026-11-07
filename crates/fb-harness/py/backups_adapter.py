"""Architecture-aware backup + local-unlock parity, using frozen services."""
import datetime as dt
import os
import json
from pathlib import Path
import shutil
import tempfile
from types import SimpleNamespace
from unittest.mock import patch
import uuid
import hashlib
import tarfile
from django.db import connection
from django.conf import settings

def select_database(name):
    connection.close()
    connection.settings_dict['NAME'] = name
    settings.DATABASES['default']['NAME'] = name

def history(scenario, count):
    snapshots, active = {}, []
    for index, step in enumerate(scenario['steps'][:count]):
        if step['op'] == 'backups_restore':
            active = list(snapshots[step['backup']])
        active.append(index)
        if step['op'] == 'backups_snapshot':
            snapshots[step['ref']] = list(active)
    return active

def identity(runner):
    if not hasattr(runner, 'restored_identity'):
        return None
    with connection.cursor() as cursor:
        cursor.execute('SELECT current_database()')
        selected = cursor.fetchone()[0]
    return dict(runner.restored_identity, selected=selected, history=history(runner.scenario,len(runner.outcomes)))

def cleanup(runner):
    if not hasattr(runner, 'backups_targets'):
        return
    select_database(runner.backups_live)
    for target in runner.backups_targets:
        core.drop_database(target)


from django.core.exceptions import ValidationError
from django.test import override_settings
from accounts import throttle
from backups import core


def normalize(runner, data):
    companies = data['contents']['companies']
    cmap = {v: k for k, v in runner.refs['company'].items()}
    amap = {v: k for k, v in runner.refs['account'].items()}
    rows = []
    for company in companies:
        company = dict(company, id=cmap.get(company['id'], '?'),
                       accounts=[dict(a, id=amap.get(a['id'], '?')) for a in company['accounts']])
        rows.append(company)
    return {'kind': 'snapshot', 'companies': rows,
            'company_count': data['contents']['tables']['ledger_company'],
            'account_count': data['contents']['tables']['ledger_account']}


def apply_backups(runner, step, actor):
    op = step['op']
    if not op.startswith('backups_'):
        return False
    if not hasattr(runner, 'backups_results'):
        runner.backups_results = {}
        runner.backups_files = {}
        runner.backups_verified = {}
        runner.backups_targets = []
        runner.backups_live = connection.settings_dict['NAME']
        runner.backups_temp = tempfile.mkdtemp(prefix='fbv2-l4b-python-', dir=os.environ['FB_HARNESS_VAR'])
    root = Path(runner.backups_temp)
    out = root / 'out'
    out.mkdir(exist_ok=True)
    private = root / 'private'
    # The desktop has one local counter. Disable the server-only cross-address
    # cap to isolate the per-computer rules the architecture retains.
    if op == 'backups_attempt':
        request = SimpleNamespace(META={'REMOTE_ADDR': '127.0.0.1'})
        now = dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=step['now'])
        with override_settings(LOGIN_MAX_FAILURES=5, LOGIN_USER_MAX_FAILURES=100000, LOGIN_LOCKOUT_SECONDS=900), patch.object(throttle.timezone, 'now', return_value=now):
            result = throttle.attempt(request, 'synthetic-owner', lambda: True if step['correct'] else None)
        result = {'kind': 'throttle', 'authenticated': result.user is not None, 'locked': result.locked}
    else:
        with override_settings(FB_BACKUP_DIR=str(out), FB_BACKUP_TEMP_DIR=str(private)):
            if op == 'backups_live_post':
                from ledger import reports
                if not hasattr(runner, 'restored_identity'):
                    raise ValidationError('Restore a book before the live-only probe.')
                selected = connection.settings_dict['NAME']
                def balance():
                    value = reports.register(actor,runner.company(step['company']),runner.account_pk(step['account']))['closing']
                    return int(value * 100)
                try:
                    select_database(runner.backups_live)
                    before = balance()
                    runner.apply(step['draft'])
                    runner.apply(step['post'])
                    after = balance()
                finally:
                    select_database(selected)
                result = dict(kind='isolation',live_before=before,live_after=after,restored=balance())
            elif op == 'backups_retention':
                raw, keep = step['label'], step['keep']
                label = core.safe_db_label(raw)
                directory = root / f'retention-{len(runner.outcomes)}'
                directory.mkdir()
                mine = [directory / f'freedombooks-{label}-202001{d:02}T000000Z.fbbackup' for d in range(1, 6)]
                for path in mine:
                    path.write_bytes(b'synthetic')
                stat = mine[1].stat()
                core.sidecar(mine[1]).write_text(json.dumps(dict(ok=True, bytes=stat.st_size, mtime_ns=stat.st_mtime_ns)))
                (directory / f'freedombooks-{core.safe_db_label(raw + "_extra")}-20200101T000000Z.fbbackup').write_bytes(b'other')
                (directory / f'freedombooks-{label}-notstamp.fbbackup').write_bytes(b'invalid')
                (directory / f'freedombooks-{label}-20200107T000000Z.fbbackup').mkdir()
                (directory / f'freedombooks-{label}-20200106T000000Z.fbbackup').symlink_to(mine[1])
                before = core.backup_files(directory, label=label)
                removed = core.prune(directory, label, keep, protect=mine[0])
                after = core.backup_files(directory, label=label)
                all_after = core.backup_files(directory)
                assert core.verified_note(mine[1]) and not any(core.sidecar(p).exists() for p in removed)
                result = dict(kind='retention', label=label, keep=keep, before=[p.name for p in before],
                              removed=[p.name for p in removed], after=[p.name for p in after],
                              all_after=[p.name for p in all_after], notes_after=[mine[1].name])
            elif op == 'backups_snapshot':
                path = out / f"{len(runner.outcomes)}.fbbackup"
                result = core.create_backup(output=path)
                runner.backups_files[step['ref']] = path
                result = normalize(runner, result.manifest)
            elif op in ('backups_verify', 'backups_restore'):
                scratch = 'fbv2_l4b_restore_' + uuid.uuid4().hex[:16]
                path = runner.backups_files[step['backup']]
                if op == 'backups_verify':
                    restored = core.verify_backup(path, scratch=scratch, record=False)
                    runner.backups_verified[step['backup']] = restored.manifest['dump']['sha256']
                else:
                    # A NEW receipts folder, as the Rust side's receipts_target (needed once files are backed up).
                    restored = core.restore_backup(path, scratch, attachments_dir=str(root / f'restored-{len(runner.outcomes)}-attachments'))
                    runner.backups_targets.append(scratch)
                    # PostgreSQL has no per-database file. Preserve and hash the exact
                    # pg_restore input; core independently compares every restored table.
                    payload = root / f'restored-{len(runner.outcomes)}.database.dump'
                    with tarfile.open(path) as archive, archive.extractfile('database.dump') as source:
                        payload.write_bytes(source.read())
                    digest = hashlib.sha256(payload.read_bytes()).hexdigest()
                    verified = runner.backups_verified.get(step['backup'], restored.manifest['dump']['sha256'])
                    assert digest == verified
                    select_database(restored.target)
                    settings.FB_ATTACHMENTS_DIR = str(restored.attachments_dir)
                    runner.restored_identity = dict(restore_step=len(runner.outcomes),backup=step['backup'],target=restored.target,live=runner.backups_live,sha256=digest,verified_sha256=verified,hash_kind='pg-restore-input',hash_path=str(payload))
                result = normalize(runner, restored.manifest)
            elif op == 'backups_refuse_existing':
                # A dedicated, unconfigured occupied target reaches the same
                # existence guard as Rust. Always drop this synthetic database.
                scratch = 'fbv2_l4b_restore_' + uuid.uuid4().hex[:16]
                from psycopg import sql
                with core.connect(core.db_settings(), autocommit=True) as conn:
                    conn.execute(sql.SQL('CREATE DATABASE {}').format(sql.Identifier(scratch)))
                try:
                    try:
                        core.restore_backup(runner.backups_files[step['backup']], scratch, attachments_dir=str(root / f'occupied-{len(runner.outcomes)}-attachments'))
                    except core.BackupError as exc:
                        if 'database already exists' not in str(exc):
                            raise ValidationError('Wrong existing-target refusal: ' + str(exc)) from None
                        raise ValidationError('Restore only goes into a NEW target.') from None
                    raise ValidationError('Existing target was not protected.')
                finally:
                    core.drop_database(scratch)
            else:
                raise ValidationError('Unknown backups operation.')
    runner.backups_results[str(len(runner.outcomes))] = result
    return True
