import { describe, expect, it } from 'vitest';
import { dockerfileName } from './dockerfileName';

describe('dockerfileName', () => {
  it.each([
    'Dockerfile',
    'app/Dockerfile',
    'dockerfile',
    'Dockerfile.dev',
    'deploy/Dockerfile.debug',
    'Containerfile',
    'containerfile',
    'Containerfile.dev',
    'app.dockerfile',
    'DEV.Dockerfile',
    '.dockerfile',
  ])('nom de base Dockerfile/Containerfile reconnu : %s', (path) => {
    expect(dockerfileName(path)).toBe(true);
  });

  it.each([
    'Docker',
    'Dockerfiles',
    'notdockerfile',
    'docker-compose.yml',
    'x.txt',
    '',
  ])('nom de base non Dockerfile : « %s »', (path) => {
    expect(dockerfileName(path)).toBe(false);
  });
});
