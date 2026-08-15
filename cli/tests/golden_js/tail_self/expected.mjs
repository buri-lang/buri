function __cmd_x_main$main(){
  core_host$HostStdout_println([[],[]][1],[String(__cmd_x_main$sumTo(100,0)),' ',String(__cmd_x_main$fib(30,0,1)),' ',String(__cmd_x_main$countDigits(12345,0))]);
  return [0,0];
}
function __cmd_x_main$sumTo(n_0,acc_1){
  while(true){
    if(n_0===0){
      return acc_1;
    }else{
      const $t1=n_0-1;
      const $t2=acc_1+n_0;
      n_0=$t1;
      acc_1=$t2;
      continue;
    }
  }
}
function __cmd_x_main$fib(n_0,a_1,b_2){
  while(true){
    if(n_0===0){
      return a_1;
    }else{
      const $t1=n_0-1;
      const $t2=b_2;
      const $t3=a_1+b_2;
      n_0=$t1;
      a_1=$t2;
      b_2=$t3;
      continue;
    }
  }
}
function __cmd_x_main$countDigits(n_0,acc_1){
  while(true){
    if(n_0<10){
      return acc_1+1;
    }else{
      const $t1=$divi(n_0,10);
      const $t2=acc_1+1;
      n_0=$t1;
      acc_1=$t2;
      continue;
    }
  }
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
