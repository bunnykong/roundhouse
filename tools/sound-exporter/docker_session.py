#!/usr/bin/env python3
"""Exclusive, disposable Discourse DB session; removes only containers it created."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import time

LOCK = os.environ.get('SOUND_DOCKER_LOCK', '/tmp/roundhouse-docker.lock')


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--work', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--selection-dir', type=Path, help='fixed mapping and generated runtime source; defaults to session/discourse')
    p.add_argument('--interval', action='store_true')
    p.add_argument('--force-logging', action='store_true', help='app-supported DISCOURSE_LOG_SIDEKIQ=1 for other tests')
    p.add_argument('tests', nargs='*', default=['spec/jobs/jobs_base_spec.rb:169'])
    a = p.parse_args()
    work, out = a.work.resolve(), a.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    app = work / 'runtime/discourse'
    names = ['sound-trace-pg-' + str(os.getpid()), 'sound-trace-redis-' + str(os.getpid())]
    images = [
        'pgvector/pgvector@sha256:cf134a767f474095eeba57e0117be8e568e011a63f33fbf252f14c9b760f8e6f',
        'valkey/valkey@sha256:49ccaa10c11575272d4e70b7262b4fdfdbb88fa729147ec6730a190c9cdf3a30',
    ]
    created, commands, started = [], [], time.monotonic()
    test_status, cleanup_errors = 2, []
    env = dict(os.environ)

    def run(cmd, *, input=None, log=None, check=True, command_env=None, cwd=None):
        tick = time.monotonic()
        result = subprocess.run(cmd, input=input, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                env=command_env or env, cwd=cwd, text=True)
        commands.append(dict(command=cmd, seconds=time.monotonic()-tick, exit=result.returncode))
        if log:
            (out / log).write_text(result.stdout)
        elif result.stdout:
            print(result.stdout[-2000:], flush=True)
        if check and result.returncode:
            raise RuntimeError('command failed; see ' + str(out / (log or 'session.log')))
        return result

    print('waiting for exclusive Docker lock', flush=True)
    with open(LOCK, 'a+') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        print('exclusive Docker lock acquired', flush=True)
        try:
            run(['docker', 'run', '--name', names[0], '-d', '-p', '127.0.0.1::5432',
                 '-e', 'POSTGRES_PASSWORD=sound-traces-local', images[0]], log='postgres-create.log')
            created.append(names[0])
            run(['docker', 'run', '--name', names[1], '-d', '-p', '127.0.0.1::6379', images[1],
                 'valkey-server', '--save', '', '--appendonly', 'no'], log='redis-create.log')
            created.append(names[1])
            for i in range(80):
                if run(['docker', 'exec', names[0], 'pg_isready', '-h', '127.0.0.1', '-U', 'postgres'],
                       check=False, log='postgres-ready.log').returncode == 0:
                    break
                time.sleep(0.25)
            else:
                raise RuntimeError('postgres did not become ready')
            run(['docker', 'exec', names[1], 'valkey-cli', 'ping'], log='redis-ready.log')
            pg_port = run(['docker', 'port', names[0], '5432/tcp'], log='postgres-port.log').stdout.strip().rsplit(':',1)[1]
            redis_port = run(['docker', 'port', names[1], '6379/tcp'], log='redis-port.log').stdout.strip().rsplit(':',1)[1]
            run(['docker', 'exec', names[0], 'createdb', '-U', 'postgres', 'sound_traces_discourse'])
            run(['docker', 'exec', '-i', names[0], 'psql', '-U', 'postgres', '-d', 'sound_traces_discourse',
                 '-v', 'ON_ERROR_STOP=1'], input=(app/'db/structure.sql').read_text(), log='structure-load.log')
            run(['docker', 'exec', names[0], 'psql', '-U', 'postgres', '-d', 'sound_traces_discourse',
                 '-c', "INSERT INTO schema_migration_details (version, created_at) SELECT version, NOW() FROM schema_migrations ON CONFLICT DO NOTHING"],
                log='migration-details.log')
            env.update(PGHOST='127.0.0.1', PGPORT=pg_port, PGUSER='postgres', PGPASSWORD='sound-traces-local',
                RAILS_ENV='test', RACK_ENV='test', RAILS_DB='sound_traces_discourse',
                DISCOURSE_REDIS_HOST='127.0.0.1', DISCOURSE_REDIS_PORT=redis_port,
                DISCOURSE_MESSAGE_BUS_REDIS_HOST='127.0.0.1', DISCOURSE_MESSAGE_BUS_REDIS_PORT=redis_port,
                DISCOURSE_LOAD_PLUGINS='0', LOAD_PLUGINS='0', DISABLE_BOOTSNAP='1',
                BUNDLE_GEMFILE=str(work/'runtime/Gemfile'), BUNDLE_WITHOUT='development',
                SOUND_APP=str(app), SOUND_PINNED_APP=str(work/'lab/corpus/apps/discourse'),
                SOUND_LAB=str(work/'lab'), SOUND_TRACE=str(out/'trace.jsonl'),
                SOUND_SELECTION_DIR=str(a.selection_dir.resolve() if a.selection_dir else work/'session/discourse'))
            if a.interval:
                env['DISCOURSE_LOG_SIDEKIQ_INTERVAL'] = '60'
            run(['ruby', str(work/'adapter/seed.rb')], log='seed.log', command_env=env, cwd=app)
            if a.force_logging:
                env['DISCOURSE_LOG_SIDEKIQ'] = '1'
            tests = a.tests or ['spec/jobs/jobs_base_spec.rb:169']
            cmd = ['ruby', str(work/'adapter/app_record.rb'), *tests, '--seed', '20261010', '--format', 'documentation']
            result = run(cmd, log='rspec.log', command_env=env, cwd=app, check=False)
            test_status = result.returncode
            run(['docker', 'image', 'inspect', *images, '--format', '{{json .RepoDigests}}'], log='image-digests.log')
            (out/'environment.json').write_text(json.dumps({k:v for k,v in env.items()
                if k.startswith(('PG','DISCOURSE_','RAILS_','RACK_','SOUND_','BUNDLE_','RBENV_'))}, indent=2)+'\n')
            print('RSpec exit ' + str(result.returncode) + '; ' + str(out/'rspec.log'), flush=True)
        finally:
            for name in reversed(created):
                run(['docker', 'logs', name], check=False, log=name+'.log')
                removed = run(['docker', 'rm', '-f', '-v', name], check=False, log=name+'-remove.log')
                if removed.returncode:
                    cleanup_errors.append(name)
            (out/'receipt.json').write_text(json.dumps(dict(lock=LOCK, containers=names, created=created,
                commands=commands, test_exit=test_status, cleanup_errors=cleanup_errors,
                seconds=time.monotonic()-started), indent=2)+'\n')
            print('Docker cleanup recorded; releasing Docker lock', flush=True)
    return 2 if cleanup_errors else test_status


if __name__ == '__main__':
    sys.exit(main())
