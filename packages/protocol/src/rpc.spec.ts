import { describe, expect, it } from 'vitest';
import { jsonrpcCode, PROTOCOL_VERSION, vnlCode } from './rpc';

describe('rpc constants', () => {
  it('PROTOCOL_VERSION == 1', () => {
    expect(PROTOCOL_VERSION).toBe(1);
  });

  it('jsonrpcCode values', () => {
    expect(jsonrpcCode.PARSE_ERROR).toBe(-32700);
    expect(jsonrpcCode.METHOD_NOT_FOUND).toBe(-32601);
    expect(jsonrpcCode.SERVER_ERROR).toBe(-32000);
  });

  it('vnlCode keys and values', () => {
    expect(vnlCode.MALFORMED_REQUEST).toBe('VNL-RPC-000');
    expect(vnlCode.NOT_INITIALIZED).toBe('VNL-RPC-001');
    expect(vnlCode.BUSY).toBe('VNL-RPC-002');
    expect(vnlCode.UNKNOWN_PROTOCOL_VERSION).toBe('VNL-RPC-003');
    expect(vnlCode.METHOD_NOT_FOUND).toBe('VNL-RPC-004');
    expect(vnlCode.CONVERSATION_NOT_FOUND).toBe('VNL-RPC-005');
    expect(vnlCode.CONFIG_ERROR).toBe('VNL-RPC-006');
    expect(vnlCode.CONVERSATION_STORAGE_ERROR).toBe('VNL-RPC-007');
    expect(vnlCode.NO_AGENT_RESOLVED).toBe('VNL-RPC-008');
    expect(vnlCode.TURN_EXECUTION_ERROR).toBe('VNL-RPC-009');
    expect(vnlCode.K8S_ERROR).toBe('VNL-RPC-010');
  });

  it('vnlCode has exactly 11 keys', () => {
    const keys = Object.keys(vnlCode);
    expect(keys.length).toBe(11);
  });
});