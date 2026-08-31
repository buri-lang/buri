const $k0=[0,0];
function __cmd_x_main_buri$main(){
  const text_2=String(__cmd_x_main_buri$code('get'))+' '+String(__cmd_x_main_buri$code('delete'))+' '+String(__cmd_x_main_buri$code('nope'));
  const self_3=$host_HostStdout_println([[],[]][1],text_2);
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  return $k0;
}
function __cmd_x_main_buri$code(name_0){
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
