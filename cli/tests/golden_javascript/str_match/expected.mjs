const $k0=[0,0];
function __cmd_x_main$main(){
  $host_HostStdout_println([[],[]][1],String(__cmd_x_main$code('get'))+' '+String(__cmd_x_main$code('delete'))+' '+String(__cmd_x_main$code('nope')));
  return $k0;
}
function __cmd_x_main$code(name_0){
  switch(name_0){
    case 'get':
      return 1;
    case 'put':
      return 2;
    case 'post':
      return 3;
    case 'patch':
      return 4;
    case 'delete':
      return 5;
    case 'head':
      return 6;
    default:
      return 0;
  }
}
