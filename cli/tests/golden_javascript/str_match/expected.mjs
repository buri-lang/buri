const $k0=[0,0];
function __cmd_x_main$main(){
  $host_HostStdout_println([],String(__cmd_x_main$code('get'))+' '+String(__cmd_x_main$code('delete'))+' '+String(__cmd_x_main$code('nope')));
  return $k0;
}
function __cmd_x_main$code(name_0){
  switch(name_0){
    case 'get':
      return 1n;
    case 'put':
      return 2n;
    case 'post':
      return 3n;
    case 'patch':
      return 4n;
    case 'delete':
      return 5n;
    case 'head':
      return 6n;
    default:
      return 0n;
  }
}
