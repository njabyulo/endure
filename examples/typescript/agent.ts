import { greeting } from './greeting.ts';

const message: string = await new Promise(resolve => {
  setTimeout(resolve, 25, greeting('Ondure'));
});

console.log(message);
