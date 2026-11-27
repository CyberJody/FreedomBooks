'use strict';

const messages = {
  app_not_running: 'Open FreedomBooks on this computer. If you already sent a change, check current state before trying again.',
  books_closed: 'Open the approved books in FreedomBooks on this computer and try again.',
  paused_signed_out: 'Sign in as the approving user and open the approved company in FreedomBooks.',
  approval_gone: 'This approval is unavailable for these books. Reopen the approved books if you switched books. Make a new connection in FreedomBooks Settings to use these books.',
  keyring_unavailable: 'Unlock the desktop keyring on this computer and try again.',
  busy: 'The books are busy. Wait for the current operation to finish and try again.',
  outcome_unknown: 'FreedomBooks could not confirm the outcome. Query current state before doing anything again. Do not retry the change blindly.',
  connection_changed: 'The connection changed. Read the connection file again and check current state before retrying.',
  operation_refused: 'Check the operation, permission, arguments and current state before retrying.',
  forbidden: 'Use the FreedomBooks local AI launcher to reach this endpoint.',
  invalid_request: 'Send one valid JSON request with the required operation and arguments.',
  not_found: 'Use an operation from the FreedomBooks AI catalogue.',
  json_required: 'Send the request as application/json.',
  request_too_large: 'Send a smaller request.',
  response_too_large: 'Request a smaller page of results.',
  method_not_found: 'Use a supported MCP method.',
};
function refusal(code, message = messages[code]) {
  return Object.assign(new Error(message), {code});
}
function detail(error) {
  return Object.hasOwn(messages,error?.code) ? {code:error.code,message:error.message} :
    {code:'operation_refused',message:messages.operation_refused};
}
function nativeError(error) {
  if (Object.hasOwn(messages,error?.code)) return error;
  if (error?.code === 'SESSION_CHANGED') return refusal('paused_signed_out');
  if (error?.message === 'BUSY') return refusal('busy');
  if (error?.message === 'anchor_unavailable') return refusal('busy','Could not read the local AI approval right now. Check access to the application data folder and try again.');
  if (error?.message === 'LOCKED') return refusal('books_closed');
  if (['books_closed','approval_gone','paused_signed_out','outcome_unknown'].includes(error?.message)) return refusal(error.message);
  return refusal('operation_refused');
}
module.exports={refusal,detail,nativeError};
